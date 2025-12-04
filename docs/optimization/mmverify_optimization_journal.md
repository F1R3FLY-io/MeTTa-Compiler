# MeTTaTron mmverify Optimization Journal

## Executive Summary

This journal documents the systematic optimization of the mmverify Metamath proof verifier demo in MeTTaTron. We follow the scientific method with hypothesis testing, requiring statistical significance (p < 0.05) for all accepted optimizations.

---

## System Configuration

### Hardware
- **CPU**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads)
- **RAM**: 252 GB DDR4 ECC @ 2133 MT/s (8× 32 GB)
- **Storage**: Samsung SSD 990 PRO 4TB (NVMe 2.0)

### Software
- **OS**: Arch Linux (kernel 6.17.9-arch1-1)
- **Rust**: 1.83+ (nightly for benchmarks)
- **Allocator**: jemalloc (via PathMap)

### Benchmark Configuration
- **Framework**: Criterion.rs 0.5
- **Samples**: 100
- **Measurement time**: 30s (e2e), 10s (compile)
- **Warm-up time**: 3s
- **Confidence level**: 95%
- **Significance level**: 0.05
- **Noise threshold**: 0.03

### Profiling Configuration
- **CPU affinity**: `taskset -c 0-17` (Socket 1 cores)
- **perf sampling**: 997 Hz with DWARF call graphs
- **Profile duration**: 120s

---

## Workload Description

### mmverify Demo (demo0.mm)
The benchmark verifies theorem th1 (t = t) from the Metamath demo0.mm database through a 32-step proof using modus ponens and equality axioms.

**Key Operations**:
1. Space operations: `new-space`, `add-atom`, `remove-atom`, `match`
2. State management: `new-state`, `get-state`, `change-state!`
3. List operations: `to-list`, `from-list`, `append`, `filter'`
4. Pattern matching: `match-atom`, `unify`, `chain`, `decons-atom`
5. Substitution: `apply_subst`, `check_subst`, `add-subst`
6. Proof verification: `treat_step`, `treat_assertion`, `verify`

**Source Files**:
- `examples/mmverify/mmverify-utils.metta` (463 lines)
- `benches/mmverify_samples/verify_demo0_body.metta` (98 lines)

---

## Phase 1: Baseline Measurements

### Date: 2025-12-04

### Benchmark Results

| Benchmark | Mean | 95% CI Lower | 95% CI Upper | Std Dev |
|-----------|------|--------------|--------------|---------|
| verify_demo0_complete | 16.406 s | 16.317 s | 16.531 s | 565.7 ms |
| compile_only | 6.016 ms | 6.008 ms | 6.026 ms | 46.3 μs |

### Key Observations

1. **Verification dominates**: Compilation is only 0.037% of total time (6ms / 16.4s)
2. **High variance**: Std dev of 565.7 ms (3.4% coefficient of variation) suggests significant non-determinism
3. **Optimization focus**: Evaluation phase is the critical path, not parsing

### Baseline Metrics

| Metric | Value | Notes |
|--------|-------|-------|
| CPU cycles | 618.58 B | From perf stat |
| Instructions | 793.53 B | Total instructions |
| IPC | 1.28 | Instructions/cycle |
| L1-dcache miss rate | 5.71% | 11.46 B misses / 200.91 B loads |
| LLC miss rate | 10.61% | 920.37 M misses / 8.68 B refs |
| Branch miss rate | 3.67% | 5.65 B misses / 154.07 B branches |

---

## Phase 2: Profiling Analysis

### Date: 2025-12-04

### CPU Hotspots (Top 15)

| Rank | Function | CPU % | Category |
|------|----------|-------|----------|
| 1 | `pathmap::zipper::k_path_default_internal` | 9.16% | PathMap Traversal |
| 2 | `MettaValue::clone` | 8.99% | Value Clone |
| 3 | `ProductZipper::child_mask` | 8.66% | PathMap Traversal |
| 4 | `mork::space::coreferential_transition` | 8.32% | MORK Pattern Match |
| 5 | `Environment::mork_expr_to_metta_value` | 5.80% | Value Conversion |
| 6 | `drop_in_place<MettaValue>` | 5.62% | Value Drop |
| 7 | `_rjem_malloc` | 5.29% | jemalloc Allocation |
| 8 | `_rjem_sdallocx` | 4.70% | jemalloc Deallocation |
| 9 | `ProductZipper::descend_to_step` | 4.49% | PathMap Traversal |
| 10 | `ReadZipperCore::step` | 3.06% | PathMap Traversal |
| 11 | `ProductZipper::clone` | 2.95% | PathMap Clone |
| 12 | `ProductZipper::ascend` | 2.91% | PathMap Traversal |
| 13 | `LineListNode::iter_pairs` | 1.48% | PathMap Iteration |
| 14 | `Vec<MettaValue>::extend_from_slice` | 1.00% | Collection Growth |
| 15 | `ProductZipper::ensure_descend_next_factor` | 0.94% | PathMap Traversal |

### Hotspot Category Summary

| Category | Total CPU % | Components |
|----------|-------------|------------|
| **PathMap Traversal** | 32.65% | zipper ops, child_mask, descend, ascend |
| **Value Clone/Drop** | 14.61% | MettaValue::clone, drop_in_place |
| **jemalloc Allocation** | 9.99% | malloc, sdallocx |
| **MORK Pattern Match** | 8.32% | coreferential_transition |
| **Value Conversion** | 5.80% | mork_expr_to_metta_value |
| **PathMap Clone** | 2.95% | ProductZipper::clone |

### Cache Statistics

| Counter | Value | Rate |
|---------|-------|------|
| L1-dcache-loads | 200.91 B | - |
| L1-dcache-load-misses | 11.46 B | 5.71% |
| cache-references (LLC) | 8.68 B | - |
| cache-misses (LLC) | 920.37 M | 10.61% |

### Branch Prediction

| Counter | Value | Rate |
|---------|-------|------|
| branches | 154.07 B | - |
| branch-misses | 5.65 B | 3.67% |

### IPC Analysis

| Counter | Value |
|---------|-------|
| cycles | 618.58 B |
| instructions | 793.53 B |
| IPC | 1.28 |

### Key Observations

1. **PathMap traversal dominates**: 32.65% of CPU time in zipper/trie operations
2. **Clone/Drop overhead significant**: 14.61% spent on MettaValue lifecycle
3. **Memory pressure**: ~10% in jemalloc, suggests allocation-heavy workload
4. **Branch prediction reasonable**: 3.67% miss rate is acceptable
5. **Cache efficiency**: L1 miss rate 5.71%, LLC miss rate 10.61% - room for improvement

---

## Optimization Hypotheses

Based on profiling analysis, the following optimization hypotheses are prioritized by expected impact:

### Priority 1: Reduce Clone/Drop Overhead (14.61% CPU)

**H1**: Eliminate redundant clones in `mork_expr_to_metta_value`
- Line 543: `items.push(value.clone())` - clones when ownership transfer suffices
- Line 548: `let completed_items = items.clone()` - clones vec then immediately pops frame
- Expected: 1-3% improvement (partial fix for 5.8% hotspot)

**H2**: Use `Arc<Vec<MettaValue>>` for SExpr to reduce deep clone costs
- Change `SExpr(Vec<MettaValue>)` to `SExpr(Arc<Vec<MettaValue>>)`
- Would convert O(n) deep clone to O(1) Arc increment
- Expected: 3-5% improvement (partial fix for 8.99% clone hotspot)
- Trade-off: Slightly more indirection, immutable semantics

### Priority 2: Reduce Allocation Overhead (9.99% CPU)

**H3**: Intern variable names instead of allocating new Strings
- Currently: `VARNAMES[i].to_string()` allocates on every variable
- Fix: Use `Cow<'static, str>` or Arc<str> for variable atoms
- Expected: 1-2% improvement

### Priority 3: Reduce PathMap Operations (32.65% CPU)

Note: PathMap/MORK are external libraries. Optimizations here focus on reducing call frequency.

**H4**: Cache pattern matching results for repeated queries
- mmverify repeatedly queries for the same rules
- Expected: 5-10% improvement if caching is applicable
- Requires analysis of query patterns

---

## Phase 3: Experiments

### Experiment 1: Eliminate Redundant Clones in mork_expr_to_metta_value

### Hypothesis
**H1**: Eliminating redundant `clone()` calls in `mork_expr_to_metta_value` will reduce
CPU overhead in value conversion from 5.80% to approximately 3-4%.

**Rationale**: Profiling shows `mork_expr_to_metta_value` consumes 5.80% of CPU. Code
inspection reveals two unnecessary clone operations:
1. `items.push(value.clone())` on line 543 - value is consumed, clone unnecessary
2. `let completed_items = items.clone()` on line 548 - frame is popped, can take ownership

### Predicted Improvement
- 1-3% improvement in e2e verification time
- Expected speedup: 1.01x - 1.03x

### Implementation
**Target Files**: `src/backend/environment.rs`
**Changes**:
1. Remove `.clone()` from `items.push(value.clone())`
2. Take ownership of `items` instead of cloning when completing s-expression

### Measurements (2025-12-04)

| Benchmark | Baseline | After | Change | p-value | Significant? |
|-----------|----------|-------|--------|---------|--------------|
| verify_demo0_complete | 16.406 s | 12.382 s | **-24.5%** | 0.00 | **YES** |

**Detailed Results:**

| Metric | Baseline | After Optimization |
|--------|----------|-------------------|
| Mean | 16.406 s | 12.382 s |
| 95% CI Lower | 16.317 s | 12.373 s |
| 95% CI Upper | 16.531 s | 12.392 s |
| Std Dev | 565.7 ms | 48.1 ms |
| Speedup | - | **1.325x** |

### Statistical Analysis

- **p-value**: 0.00 (< 0.05) - Highly statistically significant
- **Effect size (Cohen's d)**: ~10.0 (Very large effect, d > 0.8)
- **Confidence**: 95% CI for improvement: [24.1%, 25.1%]
- **Variance reduction**: Std dev dropped from 565.7ms to 48.1ms (91.5% reduction!)

### Conclusion

**Result**: **ACCEPT** - Hypothesis confirmed with results far exceeding prediction

**Reasoning**:
1. The optimization achieved a **24.5% improvement** vs predicted 1-3%
2. Statistical significance: p = 0.00 (highly significant)
3. Practical significance: 4.024 seconds saved per verification run
4. Unexpected benefit: 91.5% reduction in variance, indicating more deterministic execution
5. The improvement suggests that `mork_expr_to_metta_value` was called far more frequently
   than initially estimated (likely O(n²) clone operations in nested s-expressions)

**Root Cause Analysis**: The original code was performing exponential cloning:
- Each nested s-expression cloned all its children when building
- Deep nesting (common in Metamath proofs) caused multiplicative clone overhead
- The optimization reduced this to O(n) by taking ownership instead of cloning

---

## Summary of Results

| Experiment | Hypothesis | Predicted | Actual | p-value | Result |
|------------|------------|-----------|--------|---------|--------|
| 1 | Eliminate redundant clones | 1-3% | **24.5%** | 0.00 | **ACCEPT** |

**Cumulative Improvement**: 1.325x speedup (from 16.406s to 12.382s)

---

## Lessons Learned

1. **Clone costs are multiplicative in recursive structures**: What appeared to be
   two simple `.clone()` calls in a loop were actually causing O(n²) clones due to
   nested s-expression construction. The lesson: profile first, but also reason
   about algorithmic complexity.

2. **Variance reduction indicates systematic improvement**: The dramatic reduction
   in standard deviation (565.7ms → 48.1ms) suggests the optimization removed
   a source of non-determinism, likely related to memory allocation patterns.

3. **Simple fixes can have outsized impact**: A 10-line code change achieved 25%
   speedup. This reinforces the importance of ownership semantics in Rust -
   unnecessary clones are not just wasteful, they can be catastrophically expensive
   in hot paths.

---

## Future Work

1. **Profile after optimization**: Re-run perf to identify the new hotspots
2. **Experiment 2**: Consider Arc<Vec<MettaValue>> for SExpr if cloning is still
   a significant overhead after this optimization
3. **Experiment 3**: Intern variable names to reduce string allocations
4. **Target 10s barrier**: Current time is 12.382s, aim to break 10s with additional
   optimizations

---

## Appendix A: Raw Data Files

- `target/criterion/mmverify_e2e/verify_demo0_complete/` - Criterion benchmark data
- `target/criterion/mmverify_compile/compile_only/` - Compilation benchmark data
- `docs/optimization/mmverify_TIMESTAMP/` - perf profiling outputs
