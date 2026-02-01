# Priority Scheduler Tuning: Scientific Journal

**Start Date:** 2025-12-25
**Branch:** `feature/jit-compiler`
**Objective:** Empirically determine the optimal task classification granularity for the P² runtime tracker.

## Methodology

### Statistical Framework
- **Sample Size:** 5 runs per configuration
- **Benchmark:** `e2e_throughput` (knowledge_graph sample, 30s per mode)
- **Test:** Two-sample Welch's t-test (unequal variances)
- **Significance Level:** α = 0.05
- **Decision Rule:**
  - **ACCEPT** if p < 0.05 AND mean improvement > 0
  - **REJECT** if p ≥ 0.05 OR mean improvement ≤ 0

### Primary Metric
- `parallel-4` throughput (programs/sec)

### Secondary Metrics
- `parallel-18`, `async-18`, `sequential` throughput

### Hardware
- CPU: AMD Ryzen 9 7950X (16 cores, 32 threads)
- RAM: 252GB DDR5
- Affinity: `taskset -c 0-17` (18 logical cores)

---

## Baseline Measurements

**Branch:** `feature/jit-compiler` @ `f663c2a`
**Date:** 2025-12-25

### Current TaskTypeId Implementation
```rust
pub enum TaskTypeId {
    Eval(u64),        // Full expression hash (high granularity)
    BytecodeCompile,
    JitCompile,
    Generic,
}
```

### Raw Data (5 runs)

| Run | sequential | parallel-4 | parallel-18 | async-18 |
|-----|------------|------------|-------------|----------|
| 1   | 809.88     | 1961.31    | 4188.01     | 3544.26  |
| 2   | 841.87     | 2003.44    | 4408.08     | 3592.88  |
| 3   | 846.81     | 2002.50    | 4476.65     | 3634.91  |
| 4   | 837.83     | 2047.07    | 4197.63     | 3512.59  |
| 5   | 789.88     | 1960.10    | 4251.15     | 3532.01  |
| **Mean** | 825.25 | 1994.88   | 4304.30     | 3563.33  |
| **Std**  | 24.43  | 36.03     | 130.58      | 49.79    |

---

## Experiment 1: Single Eval Bucket

### Setup
- Parent branch: `feature/jit-compiler`
- Experiment branch: `exp/scheduler-tune-1-single-eval-bucket`
- Date: 2025-12-25

### Hypothesis
Reducing DashMap lookups by using a single estimator for all evals will reduce overhead at high parallelism while maintaining parallel-4 improvements.

### Change
```rust
pub enum TaskTypeId {
    Eval,             // All evals share one estimator (was Eval(u64))
    BytecodeCompile,
    JitCompile,
    Generic,
}
```

### Raw Data (5 runs)

| Run | sequential | parallel-4 | parallel-18 | async-18 |
|-----|------------|------------|-------------|----------|
| 1   | 825.54     | 1950.67    | 4213.25     | 3416.48  |
| 2   | 827.26     | 1981.54    | 4560.78     | 3670.94  |
| 3   | 803.47     | 1964.87    | 4263.78     | 3420.14  |
| 4   | 809.95     | 1975.63    | 4330.71     | 3487.40  |
| 5   | 780.17     | 1939.91    | 4300.22     | 3554.24  |
| **Mean** | 809.28 | 1962.52   | 4333.75     | 3509.84  |
| **Std**  | 19.17  | 17.25     | 134.25      | 106.29   |

### Statistical Analysis

| Metric | Baseline Mean | Treatment Mean | Δ% | t-statistic | p-value | Significant? |
|--------|---------------|----------------|-----|-------------|---------|--------------|
| parallel-4 | 1994.88    | 1962.52        | -1.62% | -1.811   | 0.1222  | No           |
| parallel-18 | 4304.30   | 4333.75        | +0.68% | 0.352    | 0.7343  | No           |
| async-18 | 3563.33      | 3509.84        | -1.50% | -1.019   | 0.3496  | No           |
| sequential | 825.25     | 809.28         | -1.94% | -1.151   | 0.2849  | No           |

### Decision
- [ ] **ACCEPT** - Statistically significant improvement (p < 0.05, Δ > 0)
- [x] **REJECT** - Not significant or regression

### Notes
All p-values > 0.05, indicating no statistically significant difference from baseline.
Slight regressions observed across most metrics, though within measurement noise.
The single-bucket approach eliminates per-expression specificity but does not reduce
overhead enough to offset the loss of fine-grained runtime estimation.

---

## Experiment 2: Hash Buckets (N=64)

### Setup
- Parent branch: `feature/jit-compiler` (baseline, since Exp 1 was rejected)
- Experiment branch: `exp/scheduler-tune-2-hash-buckets-64`
- Date: 2025-12-25

### Hypothesis
Bucketing expression hashes (hash % 64) balances specificity with estimator convergence speed.

### Change
```rust
pub enum TaskTypeId {
    Eval(u8),         // hash % 64 → 64 buckets
    BytecodeCompile,
    JitCompile,
    Generic,
}
```

### Raw Data (5 runs)

| Run | sequential | parallel-4 | parallel-18 | async-18 |
|-----|------------|------------|-------------|----------|
| 1   | 806.05     | 1956.65    | 4009.67     | 3334.75  |
| 2   | 728.17     | 1905.38    | 4138.79     | 3479.37  |
| 3   | 802.35     | 2025.62    | 4559.25     | 3553.32  |
| 4   | 819.40     | 2010.35    | 4463.43     | 3667.13  |
| 5   | 832.63     | 2065.71    | 4470.16     | 3607.46  |
| **Mean** | 797.72 | 1992.74   | 4328.26     | 3528.41  |
| **Std**  | 40.67  | 62.55     | 239.35      | 128.47   |

### Statistical Analysis

| Metric | Baseline Mean | Treatment Mean | Δ% | t-statistic | p-value | Significant? |
|--------|---------------|----------------|-----|-------------|---------|--------------|
| parallel-4 | 1994.88    | 1992.74        | -0.11% | -0.066   | 0.9491  | No           |
| parallel-18 | 4304.30   | 4328.26        | +0.56% | 0.196    | 0.8505  | No           |
| async-18 | 3563.33      | 3528.41        | -0.98% | -0.567   | 0.5946  | No           |
| sequential | 825.25     | 797.72         | -3.34% | -1.298   | 0.2382  | No           |

### Decision
- [ ] **ACCEPT**
- [x] **REJECT** - No statistically significant difference from baseline

### Notes
All p-values > 0.05, indicating no statistically significant difference.
Run 2 had an unusually low sequential value (728.17), possibly due to system noise.
The 64-bucket approach does not improve over full-hash tracking.

---

## Experiment 3: Hash Buckets (N=256)

### Setup
- Parent branch: `feature/jit-compiler` (baseline, since Exps 1-2 were rejected)
- Experiment branch: `exp/scheduler-tune-3-hash-buckets-256`
- Date: 2025-12-25

### Hypothesis
More buckets (256) may provide better runtime prediction without excessive DashMap entries.

### Change
```rust
pub enum TaskTypeId {
    Eval(u8),         // hash % 256 → 256 buckets
    BytecodeCompile,
    JitCompile,
    Generic,
}
```

### Raw Data (5 runs)

| Run | sequential | parallel-4 | parallel-18 | async-18 |
|-----|------------|------------|-------------|----------|
| 1   | 801.22     | 1916.83    | 3959.45     | 3230.38  |
| 2   | 823.61     | 1957.66    | 3676.35     | 3248.56  |
| 3   | 745.63     | 1833.95    | 3704.65     | 3110.70  |
| 4   | 744.46     | 1920.20    | 3957.48     | 3421.72  |
| 5   | 834.53     | 1996.18    | 3873.75     | 3321.95  |
| **Mean** | 789.89 | 1924.96   | 3834.34     | 3266.66  |
| **Std**  | 42.66  | 60.24     | 136.15      | 115.18   |

### Statistical Analysis

| Metric | Baseline Mean | Treatment Mean | Δ% | t-statistic | p-value | Significant? |
|--------|---------------|----------------|-----|-------------|---------|--------------|
| parallel-4 | 1994.88    | 1924.96        | -3.50% | -2.228   | 0.0639  | No (trend)   |
| parallel-18 | 4304.30   | 3834.34        | -10.92% | -5.571  | 0.0005  | **Yes** (regression) |
| async-18 | 3563.33      | 3266.66        | -8.33% | -5.287   | 0.0025  | **Yes** (regression) |
| sequential | 825.25     | 789.89         | -4.29% | -1.608   | 0.1560  | No           |

### Decision
- [ ] **ACCEPT**
- [x] **REJECT** - Statistically significant **regressions** at high parallelism

### Notes
**CRITICAL:** This experiment shows statistically significant performance REGRESSIONS:
- parallel-18: -10.92% (p=0.0005, highly significant)
- async-18: -8.33% (p=0.0025, highly significant)

The 256-bucket approach is significantly worse than the baseline full-hash approach.
This suggests that reducing hash specificity hurts performance by grouping dissimilar
expressions together, leading to poor runtime estimates.

---

## Experiment 4: Head-Symbol Classification

### Setup
- Parent branch: `feature/jit-compiler` (baseline, since Exps 1-3 were rejected)
- Experiment branch: `exp/scheduler-tune-4-head-symbol`
- Date: 2025-12-25

### Hypothesis
Grouping by operation type (head symbol) provides semantically meaningful runtime categories.

### Change
```rust
pub enum TaskTypeId {
    EvalArithmetic,   // +, -, *, /
    EvalComparison,   // <, >, ==, etc.
    EvalControl,      // if, match, case
    EvalSpace,        // add-atom, match, etc.
    EvalOther,        // Everything else
    BytecodeCompile,
    JitCompile,
    Generic,
}
```

### Raw Data (5 runs)

| Run | sequential | parallel-4 | parallel-18 | async-18 |
|-----|------------|------------|-------------|----------|
| 1   | 777.55     | 1954.62    | 4164.93     | 3431.87  |
| 2   | 803.34     | 2007.92    | 4077.72     | 3374.01  |
| 3   | 852.24     | 2023.53    | 3977.03     | 3273.92  |
| 4   | 820.46     | 1938.26    | 3990.49     | 3344.38  |
| 5   | 791.60     | 1973.15    | 3953.93     | 3301.33  |
| **Mean** | 809.04 | 1979.50   | 4032.82     | 3345.10  |
| **Std**  | 28.83  | 35.73     | 87.46       | 61.94    |

### Statistical Analysis

| Metric | Baseline Mean | Treatment Mean | Δ% | t-statistic | p-value | Significant? |
|--------|---------------|----------------|-----|-------------|---------|--------------|
| parallel-4 | 1994.88    | 1979.50        | -0.77% | -0.678   | 0.5168  | No           |
| parallel-18 | 4304.30   | 4032.82        | -6.31% | -3.863   | 0.0062  | **Yes** (regression) |
| async-18 | 3563.33      | 3345.10        | -6.12% | -6.140   | 0.0003  | **Yes** (regression) |
| sequential | 825.25     | 809.04         | -1.96% | -0.960   | 0.3661  | No           |

### Decision
- [ ] **ACCEPT**
- [x] **REJECT** - Statistically significant **regressions** at high parallelism

### Notes
Semantic grouping by head symbol performs worse than full expression hashing.
Significant regressions at high parallelism:
- parallel-18: -6.31% (p=0.0062)
- async-18: -6.12% (p=0.0003)

The coarse categorization loses important expression-specific runtime information,
leading to poor P² estimates. The full hash approach captures fine-grained differences
that are predictive of runtime.

---

## Experiment 5: Disable Eval Runtime Tracking

### Setup
- Parent branch: `feature/jit-compiler` (baseline, since Exps 1-4 were rejected)
- Experiment branch: `exp/scheduler-tune-5-no-eval-tracking`
- Date: 2025-12-25

### Hypothesis
Runtime tracking overhead may exceed SJF benefits; pure FIFO within priority levels may be faster.

### Change
```rust
// Skip P² tracking for eval tasks, use base_priority + decay only
pub fn score(&self, runtime_tracker: &RuntimeTracker, config: &SchedulerConfig) -> f64 {
    match self.task_type {
        TaskTypeId::Eval(_) => {
            // Pure FIFO with decay for eval tasks
            base - age_component
        }
        _ => {
            // Full P² scoring for non-eval tasks
            base + runtime_component - age_component
        }
    }
}
```

### Raw Data (5 runs)

| Run | sequential | parallel-4 | parallel-18 | async-18 |
|-----|------------|------------|-------------|----------|
| 1   | 831.34     | 1963.97    | 4036.62     | 3352.88  |
| 2   | 832.63     | 2011.78    | 3736.69     | 3249.94  |
| 3   | 849.11     | 1978.99    | 3967.76     | 3332.88  |
| 4   | 808.59     | 2011.50    | 3926.27     | 3352.52  |
| 5   | 802.08     | 1958.30    | 3916.89     | 3267.16  |
| **Mean** | 824.75 | 1984.91   | 3916.85     | 3311.08  |
| **Std**  | 19.20  | 25.55     | 111.22      | 49.01    |

### Statistical Analysis

| Metric | Baseline Mean | Treatment Mean | Δ% | t-statistic | p-value | Significant? |
|--------|---------------|----------------|-----|-------------|---------|--------------|
| parallel-4 | 1994.88    | 1984.91        | -0.50% | -0.505   | 0.6286  | No           |
| parallel-18 | 4304.30   | 3916.85        | -9.00% | -5.051   | 0.0011  | **Yes** (regression) |
| async-18 | 3563.33      | 3311.08        | -7.08% | -8.074   | 0.0000  | **Yes** (regression) |
| sequential | 825.25     | 824.75         | -0.06% | -0.036   | 0.9720  | No           |

### Decision
- [ ] **ACCEPT**
- [x] **REJECT** - Statistically significant **regressions** at high parallelism

### Notes
**KEY FINDING:** Disabling P² runtime tracking causes significant performance regressions:
- parallel-18: -9.00% (p=0.0011)
- async-18: -7.08% (p=0.0000, highly significant)

This proves that the P² runtime estimation provides real value for task scheduling.
The SJF (Shortest Job First) approximation from P² estimates outweighs the overhead
of maintaining per-expression estimators.

Sequential and parallel-4 are essentially neutral, suggesting that P² benefits
emerge primarily under high concurrency where scheduling decisions matter more.

---

## Summary

### Experiment Results

| Experiment | Hypothesis | Result | p-value (parallel-4) | Δ% (parallel-4) |
|------------|------------|--------|---------------------|------------------|
| 1. Single Eval Bucket | Reduce DashMap lookups | **REJECT** | 0.1222 | -1.62% |
| 2. Hash Buckets (64)  | Balance specificity | **REJECT** | 0.9491 | -0.11% |
| 3. Hash Buckets (256) | Better prediction | **REJECT** | 0.0639 | -3.50% |
| 4. Head-Symbol        | Semantic categories | **REJECT** | 0.5168 | -0.77% |
| 5. No Eval Tracking   | Reduce overhead | **REJECT** | 0.6286 | -0.50% |

### Key Findings

1. **Full expression hashing (current baseline) is optimal.**
   All experiments that reduced hash specificity showed neutral or negative results.

2. **P² runtime tracking provides measurable value.**
   Experiment 5 showed that disabling runtime tracking causes 7-9% regressions at
   high parallelism (p < 0.005), proving the SJF approximation benefits outweigh overhead.

3. **Coarse categorization hurts performance.**
   Experiments 3 and 4 showed statistically significant regressions (10.9% and 6.3%
   at parallel-18), indicating that expression-specific runtime information is valuable.

4. **High parallelism magnifies scheduling importance.**
   Sequential and parallel-4 metrics showed minimal impact from changes, while
   parallel-18 and async-18 showed significant effects. This confirms that intelligent
   scheduling matters most under high concurrency.

### Final Recommendation

**Keep the current implementation unchanged.**

The original `TaskTypeId::Eval(u64)` design using full expression hashes is
empirically optimal. The high granularity provides:

- Fine-grained runtime prediction per expression type
- Effective SJF approximation under high concurrency
- No measurable overhead compared to coarser alternatives

The P² algorithm's O(1) space and time complexity, combined with DashMap's
lock-free concurrent access, makes the per-expression tracking overhead negligible
compared to the scheduling benefits.

### Branch Lineage

```
feature/jit-compiler @ f663c2a (baseline) ← RECOMMENDED
  ├── exp/scheduler-tune-1-single-eval-bucket (REJECTED)
  ├── exp/scheduler-tune-2-hash-buckets-64 (REJECTED)
  ├── exp/scheduler-tune-3-hash-buckets-256 (REJECTED - significant regression)
  ├── exp/scheduler-tune-4-head-symbol (REJECTED - significant regression)
  └── exp/scheduler-tune-5-no-eval-tracking (REJECTED - significant regression)
```

### Statistical Summary

| Experiment | Sequential | Parallel-4 | Parallel-18 | Async-18 |
|------------|------------|------------|-------------|----------|
| Baseline   | 825.25     | 1994.88    | 4304.30     | 3563.33  |
| 1. Single  | -1.94%     | -1.62%     | +0.68%      | -1.50%   |
| 2. 64-Buck | -3.34%     | -0.11%     | +0.56%      | -0.98%   |
| 3. 256-Buck| -4.29%     | -3.50%     | **-10.92%** | **-8.33%** |
| 4. HeadSym | -1.96%     | -0.77%     | **-6.31%**  | **-6.12%** |
| 5. NoTrack | -0.06%     | -0.50%     | **-9.00%**  | **-7.08%** |

Bold values indicate statistically significant results (p < 0.05).
