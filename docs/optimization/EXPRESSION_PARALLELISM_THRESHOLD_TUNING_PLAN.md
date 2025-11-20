# Expression Parallelism Threshold Tuning Plan

**Date**: 2025-11-12
**Status**: Baseline Benchmarks Running
**Current Threshold**: 4 sub-expressions

---

## Executive Summary

Empirically tune the `PARALLEL_EVAL_THRESHOLD` constant (currently 4) to find the optimal point where parallelization overhead is justified by performance gains.

**Goal**: Identify the threshold value that maximizes performance across a variety of MeTTa workloads.

**Method**: Run comprehensive benchmarks with current threshold, analyze results, and identify optimal value based on empirical measurements.

---

## Current Implementation

**Location**: `src/backend/eval/mod.rs:45`

```rust
/// Threshold for parallel sub-expression evaluation
/// Only parallelize when number of sub-expressions >= this value
/// Below this threshold, sequential evaluation is faster due to parallel overhead
/// Empirically determined: parallel overhead (~50µs) vs evaluation time
const PARALLEL_EVAL_THRESHOLD: usize = 4;
```

**Parallel Evaluation Logic** (`src/backend/eval/mod.rs:193-203`):

```rust
if items.len() >= PARALLEL_EVAL_THRESHOLD {
    // Parallel evaluation for complex expressions
    items
        .par_iter()
        .map(|item: &MettaValue| eval_with_depth(item.clone(), env.clone(), depth + 1))
        .collect()
} else {
    // Sequential evaluation for simple expressions
    items
        .iter()
        .map(|item: &MettaValue| eval_with_depth(item.clone(), env.clone(), depth + 1))
        .collect()
}
```

---

## Hypothesis

**Current threshold (4)** was empirically determined based on parallel overhead (~50µs). However:

1. **Overhead may have changed** since threshold was set (code evolution, new optimizations)
2. **Workload characteristics** may favor different thresholds for different expression types
3. **Hardware-specific** factors (18-core Xeon E5-2699 v3) may require tuning

**Hypotheses to test**:

### H1: Current Threshold is Optimal
Current threshold (4) minimizes overhead while maximizing parallelism benefit across workloads.

**Expected Result**: No significant performance improvement with different thresholds

### H2: Higher Threshold is Better
Parallel overhead outweighs benefits until higher operation count (5-8).

**Expected Result**: Threshold 6-8 shows better performance than 4

### H3: Lower Threshold is Better
Recent optimizations (Phase 1+2) reduced sequential overhead, making parallelism beneficial earlier.

**Expected Result**: Threshold 2-3 shows better performance than 4

### H4: Workload-Specific Thresholds
Different expression types have different optimal thresholds.

**Expected Result**: No single threshold optimal across all workloads

---

## Benchmark Suite Analysis

### Benchmark Groups (from `benches/expression_parallelism.rs`)

#### 1. `simple_arithmetic` (Lines 85-106)
**Tests**: 2, 3, 4, 5, 6, 8, 10 operations
**Pattern**: `(+ (* 2 3) (/ 10 5) (- 8 4) ...)`
**Purpose**: Test threshold boundary with basic arithmetic

**Key Insight**: Operations around threshold (3, 4, 5) will show crossover point

#### 2. `nested_expressions` (Lines 109-130)
**Tests**: Depths 2, 3, 4, 5, 6
**Pattern**: `(+ (+ (+ 1 2) (+ 3 4)) ...)`
**Purpose**: Test deeply recursive patterns

**Key Insight**: Nested parallelism may have different overhead characteristics

#### 3. `mixed_complexity` (Lines 133-153)
**Tests**: 2, 4, 8, 12, 16, 20 operations
**Pattern**: Mix of arithmetic, nested operations, division
**Purpose**: Simulate realistic MeTTa workloads

**Key Insight**: Real-world performance indicator

#### 4. `threshold_tuning` (Lines 157-178)
**Tests**: 2, 3, 4, 5, 6, 7, 8, 10, 12, 16 operations
**Pattern**: Simple arithmetic with fine granularity around threshold
**Purpose**: **Primary metric for threshold selection**

**Key Insight**: Identifies exact crossover point where parallel > sequential

#### 5. `realistic_expressions` (Lines 182-259)
**Tests**: 3 real-world scenarios (financial calc, vector ops, complex formula)
**Pattern**: Practical MeTTa code patterns
**Purpose**: Validate threshold choice on real workloads

**Key Insight**: Must perform well on these patterns

#### 6. `parallel_overhead` (Lines 263-289)
**Tests**: 1, 2, 3, 4, 5, 6 trivial operations (just integers)
**Pattern**: `(+ 1 2 3 4 5 6)`
**Purpose**: Measure pure parallel overhead with minimal computation

**Key Insight**: Shows where overhead dominates

#### 7. `scalability` (Lines 293-313)
**Tests**: 4, 8, 16, 32, 64 operations
**Pattern**: Large arithmetic expressions
**Purpose**: Test how parallelism scales with operation count

**Key Insight**: Verify parallel benefit increases with size

---

## Analysis Methodology

### Step 1: Baseline Analysis (Current Threshold = 4)

**Metrics to collect**:
1. **Mean time** for each operation count
2. **Variance** (consistency of performance)
3. **Throughput** (operations/second)
4. **Overhead** (trivial operations baseline)

**Key Questions**:
- At what point does parallel become faster than sequential?
- How much does nesting affect the crossover?
- What is the measured overhead?

### Step 2: Identify Crossover Point

**From `threshold_tuning` benchmark**:
- Plot time vs operation count
- Identify where performance curve changes slope
- Look for threshold where parallel overhead is justified

**Expected Pattern**:
```
Time (µs)
  |
  |     /  Sequential (linear growth)
  |    /
  |   /________  Parallel (sub-linear growth after overhead)
  |  /
  |_/_____|_____|_____|_____|_____|_____|_____
   1  2  3  4  5  6  7  8  9  10  11  12  Ops
           ^
       Crossover point
```

### Step 3: Validate Across Workloads

**Check that optimal threshold performs well on**:
- `simple_arithmetic` (basic operations)
- `nested_expressions` (recursive patterns)
- `mixed_complexity` (realistic mix)
- `realistic_expressions` (practical scenarios)

**Acceptance Criteria**:
- No regression on any workload > 5%
- Measurable improvement on at least 2 workload types
- Consistent performance (low variance)

### Step 4: Test Alternative Thresholds (If Needed)

**If crossover ≠ 4**, test alternative thresholds:

**Candidates**:
- Threshold 2 (if crossover at 2-3)
- Threshold 3 (if crossover at 3-4)
- Threshold 5 (if crossover at 5-6)
- Threshold 6 (if crossover at 6-8)

**Implementation**:
1. Modify `PARALLEL_EVAL_THRESHOLD` in `src/backend/eval/mod.rs:45`
2. Rerun full benchmark suite
3. Compare against baseline

### Step 5: Statistical Analysis

**For each threshold candidate**:
1. Compare mean times across operation counts
2. Calculate speedup ratio: `time_baseline / time_candidate`
3. Perform significance testing (t-test or similar)
4. Check for workload-specific effects

**Decision Criteria**:
- Speedup > 1.05 (5% improvement minimum)
- Statistically significant (p < 0.05)
- No regressions > 5% on any workload
- Consistent across multiple runs

---

## Hardware Context

**System**: Intel Xeon E5-2699 v3 @ 2.30GHz
- **Cores**: 36 physical (72 threads with HT)
- **L1 Cache**: 1.1 MiB (data) + 1.1 MiB (instruction)
- **L2 Cache**: ~9 MB
- **L3 Cache**: ~45 MB
- **Rayon Default**: Uses all 72 threads

**Parallel Overhead Sources**:
1. **Thread spawning**: Rayon thread pool (amortized across operations)
2. **Synchronization**: Barrier/join at end of parallel section
3. **Cache coherency**: Sharing `Environment` across threads
4. **NUMA effects**: Memory access patterns across CPU cores

**Optimization Opportunities**:
- Rayon reuses thread pool (low overhead after warmup)
- `Environment` is cloned (thread-local after clone)
- 18-core allocation via `taskset -c 0-17` (Phase 1 analysis)

---

## Expected Results

### Scenario A: Threshold = 4 is Optimal

**Benchmark Results**:
- Crossover at 3-4 operations
- No significant improvement with threshold 2, 3, 5, or 6
- `threshold_tuning` shows steady performance around threshold 4

**Conclusion**: Keep current threshold

**Documentation**: Update comments with empirical validation

### Scenario B: Lower Threshold (2-3) is Better

**Benchmark Results**:
- Crossover at 2-3 operations
- Parallel overhead < sequential cost even at 3 operations
- Phase 1+2 optimizations reduced sequential overhead

**Action**: Lower threshold to 3 (or 2 if crossover at 2)

**Expected Speedup**: 1.2-1.5× for expressions with 3-4 operations

### Scenario C: Higher Threshold (5-8) is Better

**Benchmark Results**:
- Crossover at 5-6 operations
- Parallel overhead dominates until 6+ operations
- `parallel_overhead` shows high overhead even at 4 operations

**Action**: Raise threshold to 6 (or 5 if crossover at 5)

**Expected Speedup**: 1.1-1.3× for very large expressions (10+ operations)

### Scenario D: Workload-Specific Thresholds

**Benchmark Results**:
- Different optimal thresholds for different workload types
- `nested_expressions` favors lower threshold (more parallelism)
- `simple_arithmetic` favors higher threshold (overhead dominant)

**Action**:
- Option 1: Use conservative threshold (highest stable value)
- Option 2: Implement adaptive thresholding based on expression type
- Option 3: Profile-guided optimization (PGO)

**Complexity vs Benefit**: Likely stick with single threshold unless benefit > 20%

---

## Implementation Plan

### Phase 1: Baseline Analysis ✅ (In Progress)

1. **Run baseline benchmarks** (threshold = 4)
2. **Analyze results** (identify crossover point)
3. **Document findings**

**Status**: Benchmarks running with `taskset -c 0-17` for CPU affinity

**Output**: `/tmp/expression_parallelism_baseline.txt`

### Phase 2: Threshold Selection (Pending)

1. **Identify optimal threshold** from baseline analysis
2. **Test alternative thresholds** (if needed)
3. **Statistical validation**

### Phase 3: Implementation (Pending)

1. **Update `PARALLEL_EVAL_THRESHOLD`** if different from 4
2. **Run full test suite** (403 tests must pass)
3. **Rerun benchmarks** to confirm improvement

### Phase 4: Documentation (Pending)

1. **Update CHANGELOG.md** with threshold tuning results
2. **Update code comments** with empirical justification
3. **Create summary document** with before/after comparison

---

## Success Metrics

**Minimum Success Criteria**:
1. Empirically validated threshold value with data
2. No regressions > 5% on any workload
3. All 403 tests pass

**Ideal Success Criteria**:
1. Measurable improvement (5-20%) on 2+ workload types
2. Crossover point clearly identified
3. Statistical significance (p < 0.05)
4. Documented empirical justification

---

## Risk Assessment

**Low Risk**:
- Threshold tuning is parameter adjustment (no algorithm changes)
- Extensive benchmark suite covers edge cases
- Easy to revert if no improvement

**Potential Issues**:
1. **No clear crossover**: Threshold choice may be hardware/workload dependent
2. **Variance too high**: Inconsistent results make threshold selection ambiguous
3. **Workload mismatch**: Benchmark workloads don't reflect real MeTTa usage

**Mitigation**:
- Keep current threshold (4) if no clear winner
- Document findings even if no change made
- Consider adaptive thresholding as future work

---

## Next Steps

1. ⏳ **Wait for baseline benchmarks to complete** (~10-15 minutes)
2. 📊 **Analyze baseline results** (crossover point, overhead, variance)
3. 🎯 **Select optimal threshold** based on empirical data
4. 🧪 **Test alternative threshold** (if needed)
5. ✅ **Update code and documentation**

---

## Related Documents

- `docs/optimization/OPTIMIZATION_PHASES_SUMMARY.md` - Context from Phase 1-4
- `docs/optimization/PATHMAP_BATCH_API_FINDINGS.md` - PathMap optimization (separate track)
- `benches/expression_parallelism.rs` - Benchmark implementation
- `src/backend/eval/mod.rs` - Current threshold implementation

---

**End of Expression Parallelism Threshold Tuning Plan**
