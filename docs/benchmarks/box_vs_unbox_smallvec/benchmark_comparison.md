# Benchmark Comparison: Boxed vs Unboxed SmallVec

## Executive Summary

**Decision: KEEP the unboxed SmallVec implementation**

The unboxed version shows consistent performance improvements across all benchmarks, with the most significant gain in pattern matching operations (5.9% improvement). No regressions were observed.

## Detailed Results

### Pattern Matching Stress (Primary Target)
**Most Important**: This benchmark directly exercises the bindings code with 2-8 variable patterns.

| Metric   | Boxed (baseline) | Unboxed | Improvement |
|----------|------------------|---------|-------------|
| Median   | 2.007 ms         | 1.889 ms| **5.9%** ✅ |
| Mean     | 2.03 ms          | 1.907 ms| **6.1%** ✅ |
| Fastest  | 1.845 ms         | 1.726 ms| **6.5%** ✅ |
| Slowest  | 2.445 ms         | 2.23 ms | **8.8%** ✅ |

### Knowledge Graph (Real-world Pattern Matching)
Realistic queries using pattern matching with bindings.

| Metric   | Boxed (baseline) | Unboxed | Improvement |
|----------|------------------|---------|-------------|
| Median   | 910.8 µs         | 879 µs  | **3.5%** ✅ |
| Mean     | 944.7 µs         | 900.8 µs| **4.6%** ✅ |
| Fastest  | 841.3 µs         | 817.4 µs| **2.8%** ✅ |
| Slowest  | 1.367 ms         | 1.268 ms| **7.2%** ✅ |

### Fibonacci (Recursive with Bindings)
Tests recursive evaluation with variable bindings.

| Metric   | Boxed (baseline) | Unboxed | Improvement |
|----------|------------------|---------|-------------|
| Median   | 5.74 ms          | 5.629 ms| **1.9%** ✅ |
| Mean     | 5.807 ms         | 5.681 ms| **2.2%** ✅ |
| Fastest  | 5.332 ms         | 5.28 ms | **1.0%** ✅ |
| Slowest  | 6.754 ms         | 6.543 ms| **3.1%** ✅ |

### Other Benchmarks

| Benchmark                      | Boxed Median | Unboxed Median | Improvement |
|--------------------------------|--------------|----------------|-------------|
| concurrent_space_operations    | 3.437 ms     | 3.367 ms       | **2.0%** ✅ |
| constraint_search              | 2.939 ms     | 2.897 ms       | **1.4%** ✅ |
| metta_programming_stress       | 2.165 ms     | 2.117 ms       | **2.2%** ✅ |
| multi_space_reasoning          | 930.3 µs     | 912.4 µs       | **1.9%** ✅ |

## Analysis

### Performance Impact

1. **Primary Benchmark (pattern_matching_stress)**: 5.9% improvement
   - Exceeds our 3% threshold for keeping the change ✅
   - This benchmark directly tests the SmartBindings hot path
   - Improvement is consistent across all metrics (fastest, median, mean, slowest)

2. **Secondary Benchmarks**: All show improvements (1.0% - 7.2%)
   - No regressions observed ✅
   - All benchmarks improved, demonstrating broad positive impact
   - Improvements range from small (1.0% for fib fastest) to significant (7.2% for knowledge_graph slowest)

3. **Statistical Significance**:
   - 100 samples per benchmark (divan default)
   - Consistent improvements across median, mean, fastest, and slowest metrics
   - Pattern is clear: unboxed is faster

### Technical Explanation

The performance improvements are explained by:

1. **Eliminated Heap Allocation**: Transitioning from Single → Small no longer requires heap allocation for the Box
2. **Removed Pointer Indirection**: All SmallVec access operations now work directly on the data
3. **Improved Cache Locality**: Data is inline in the enum, not scattered across heap allocations
4. **Typical Case Optimization**: Most patterns use 2-5 variables (fits in SmallVec inline capacity)

### Trade-offs

**Enum Size Increase**:
- Before (boxed): 32 bytes
- After (unboxed): 608 bytes

**Why This Is Acceptable**:
- SmartBindings are primarily passed by reference (no copy overhead)
- Bindings are short-lived (created during pattern matching, dropped after)
- Stack space is not a constraint for typical pattern matching depth
- Performance gains outweigh the stack space cost

## Decision Criteria

✅ **Keep Unboxed If**:
- `pattern_matching_stress` ≥ 3% faster: **YES (5.9% faster)**
- No benchmark regresses > 2%: **YES (all improved)**

❌ **Revert to Boxed If**:
- Any benchmark regresses > 5%: **NO (none regressed)**
- Stack overflow occurs: **NO (no issues observed)**

## Recommendation

**KEEP the unboxed SmallVec implementation** (current state of the code).

### Rationale:
1. Primary benchmark improved by 5.9% (exceeds 3% threshold)
2. All other benchmarks showed improvements (1.0% - 7.2%)
3. No regressions observed
4. No stability issues (stack overflows, etc.)
5. Enum size increase is acceptable given the performance gains

### What Changed:
- `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/models/bindings.rs`
  - Line 24: `Small(Box<SmallVec<[...]>>)` → `Small(SmallVec<[...]>)`
  - Line 67: `Box::new(vec)` → `vec`

### Impact:
- Pattern matching operations are **~6% faster**
- Knowledge graph queries are **~4% faster**
- Recursive evaluations are **~2% faster**
- All workloads benefit, none are negatively affected

## Benchmark Environment

- **Timer Precision**: 12 ns
- **Samples**: 100 per benchmark
- **Iterations**: 100 per sample
- **Platform**: Linux 6.17.7-arch1-1
- **CPU**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores)
- **Build**: Release mode with target-cpu=native

## Files

- `baseline_boxed.txt`: Original benchmark results (boxed SmallVec)
- `baseline_unboxed.txt`: New benchmark results (unboxed SmallVec)
- `benchmark_comparison.md`: This analysis document
