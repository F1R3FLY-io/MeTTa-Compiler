# CPU Optimization Experiments - Scientific Ledger

## Executive Summary

This document records the scientific investigation of CPU optimizations for the MeTTa-Compiler.
All experiments follow rigorous statistical methodology with significance threshold p < 0.05.

**Status**: In Progress
**Branch**: `perf/cpu-opt-baseline`
**Date Started**: 2025-12-04
**Hardware**: Intel Xeon E5-2699 v3 @ 2.30GHz (Haswell-EP), 252GB RAM

---

## Experimental Protocol

### Statistical Framework
- **Significance threshold**: p < 0.05 (two-tailed)
- **Effect size metric**: Cohen's d
- **Practical threshold**: > 2% improvement required for acceptance
- **Tool**: Criterion.rs with 100+ iterations per benchmark

### Decision Criteria
| Condition | Action |
|-----------|--------|
| p < 0.05 AND improvement > 2% | **ACCEPT** |
| p < 0.05 AND improvement <= 2% | **REJECT** (negligible) |
| p >= 0.05 | **REJECT** (not significant) |
| Regression detected (p < 0.05) | **REJECT** (performance worse) |

### Benchmarks
1. `pattern_match` - Pattern matching performance (primary metric)
2. `rule_matching` - Rule lookup and application
3. `type_lookup` - Type system operations
4. `metta` - End-to-end evaluation

---

## Baseline Measurements

**Branch**: `perf/cpu-opt-baseline`
**Commit**: (from `feature/context-aware-fuzzy-matching`)
**Date**: 2025-12-04

### pattern_match Benchmark Results (Criterion.rs)

| Benchmark | Mean | 95% CI |
|-----------|------|--------|
| simple_variable | 194.52 ns | [194.37 ns, 194.67 ns] |
| multiple_variables_3 | 358.92 ns | [358.60 ns, 359.29 ns] |
| variable_count_scaling/1 | 199.91 ns | [199.86 ns, 199.95 ns] |
| variable_count_scaling/5 | 499.12 ns | [499.02 ns, 499.24 ns] |
| variable_count_scaling/10 | 968.81 ns | [968.34 ns, 969.32 ns] |
| variable_count_scaling/25 | 2.5910 µs | [2.5905 µs, 2.5916 µs] |
| variable_count_scaling/50 | 7.8941 µs | [7.8905 µs, 7.8978 µs] |
| nested_2_levels | 364.44 ns | [364.29 ns, 364.63 ns] |
| nesting_depth/1 | 195.10 ns | [194.99 ns, 195.23 ns] |
| nesting_depth/3 | 363.99 ns | [363.92 ns, 364.07 ns] |
| nesting_depth/5 | 530.22 ns | [530.13 ns, 530.31 ns] |
| nesting_depth/10 | 1.0219 µs | [1.0217 µs, 1.0221 µs] |
| existing_binding_simple | 220.79 ns | [220.73 ns, 220.86 ns] |
| ground_types/bool | 153.39 ns | [153.35 ns, 153.44 ns] |
| ground_types/long | 152.21 ns | [152.17 ns, 152.26 ns] |
| ground_types/float | 152.84 ns | [152.71 ns, 152.97 ns] |
| ground_types/string | 156.59 ns | [156.55 ns, 156.63 ns] |
| ground_types/atom | 158.11 ns | [158.06 ns, 158.18 ns] |
| wildcards | 211.25 ns | [211.21 ns, 211.30 ns] |
| mixed_complexity | 606.19 ns | [606.04 ns, 606.37 ns] |
| failures/type_mismatch | 139.45 ns | [139.42 ns, 139.49 ns] |

### rule_matching Benchmark Results (Criterion.rs)

| Benchmark | Mean | 95% CI |
|-----------|------|--------|
| fibonacci_lookup/10 | 759.64 µs | [758.57 µs, 760.60 µs] |
| fibonacci_lookup/50 | 2.8709 ms | [2.8601 ms, 2.8822 ms] |
| fibonacci_lookup/100 | 5.5406 ms | [5.5097 ms, 5.5698 ms] |
| fibonacci_lookup/500 | 26.762 ms | [26.582 ms, 26.934 ms] |
| fibonacci_lookup/1000 | 51.220 ms | [50.912 ms, 51.531 ms] |
| pattern_matching/simple_variable | 81.679 µs | [81.205 µs, 82.058 µs] |
| pattern_matching/nested_destructuring | 172.25 µs | [169.74 µs, 174.46 µs] |
| full_evaluation/fibonacci_10 | 323.89 µs | [323.55 µs, 324.22 µs] |
| full_evaluation/nested_let | 80.234 µs | [79.740 µs, 80.728 µs] |
| large_rule_sets/worst_case_lookup/100 | 6.2076 ms | [6.0642 ms, 6.4905 ms] |
| large_rule_sets/worst_case_lookup/500 | 30.249 ms | [29.970 ms, 30.833 ms] |
| large_rule_sets/worst_case_lookup/1000 | 60.543 ms | [59.256 ms, 61.789 ms] |
| has_sexpr_fact/query_existing_fact/100 | 695.81 ns | [695.73 ns, 695.90 ns] |

### type_lookup Benchmark Results (Criterion.rs)

| Benchmark | Mean | 95% CI |
|-----------|------|--------|
| get_type_first/10 | 1.3098 µs | [1.3075 µs, 1.3124 µs] |
| get_type_middle/10 | 4.3105 µs | [4.3089 µs, 4.3126 µs] |
| get_type_last/10 | 6.6753 µs | [6.6722 µs, 6.6792 µs] |
| get_type_missing/10 | 6.3970 µs | [6.3937 µs, 6.4013 µs] |
| get_type_first/100 | 1.3200 µs | [1.3176 µs, 1.3237 µs] |
| get_type_middle/100 | 31.327 µs | [31.242 µs, 31.470 µs] |
| get_type_last/100 | 60.930 µs | [60.791 µs, 61.094 µs] |
| get_type_missing/100 | 57.700 µs | [57.689 µs, 57.714 µs] |
| get_type_first/1000 | 1.3568 µs | [1.3557 µs, 1.3579 µs] |
| get_type_middle/1000 | 311.94 µs | [311.86 µs, 312.05 µs] |
| get_type_last/1000 | 628.70 µs | [628.37 µs, 629.17 µs] |
| get_type_missing/1000 | 593.16 µs | [592.87 µs, 593.64 µs] |

### metta Benchmark Results (divan)

| Benchmark | Median | Mean | Range |
|-----------|--------|------|-------|
| async_concurrent_space_operations | 2.71 ms | 2.772 ms | [2.541 ms, 6.035 ms] |
| async_fib | 7.302 ms | 7.363 ms | [6.884 ms, 8.273 ms] |
| async_knowledge_graph | 2.759 ms | 2.763 ms | [2.642 ms, 2.957 ms] |

### Perf Profile Summary

**Workload**: 100 iterations of `examples/advanced.metta` + `examples/simple.metta`
**Samples**: 195 (Event: cycles:Pu)

```
Top Hotspots (% overhead):
  28.55%  [kernel]                     - System calls (madvise, allocation)
  11.80%  [kernel]                     - Memory deallocation overhead
   3.30%  eval_trampoline              - Main evaluation loop
   3.30%  mork_convert::write_metta_value - MORK conversion
   1.89%  _rjem_malloc                 - jemalloc allocation
   1.89%  base_alloc_impl              - jemalloc base allocation
   1.42%  SipHash Hasher::write        - HashMap hashing (std SipHash)
   1.42%  DynamicDawgChar::insert      - DAWG dictionary insertion
   1.42%  _rjem_sdallocx               - jemalloc deallocation
   1.42%  PathMap node_key_overlap     - PathMap trie operations
```

**Key Finding**: SipHash (1.42%) confirms potential for ahash optimization.
Memory allocation/deallocation dominates (~40% combined kernel overhead).

---

## Experiment 1: ahash-hasher

**Branch**: `perf/cpu-opt-ahash`
**Hypothesis**: Replacing std HashMap hasher with ahash will improve HashMap operations by 30-50%
**Status**: COMPLETED

### Changes
- Add `ahash = { version = "0.8", optional = true }` to Cargo.toml
- Create `src/backend/hash_utils.rs` with FastHashMap/FastHashSet type aliases
- Replace HashMap imports in `environment.rs` (rule_index, multiplicities, bindings, named_spaces, states)
- Replace HashSet in ScopeTracker

### Results

**pattern_match benchmarks** (uses SmartBindings, not HashMap):

| Benchmark | Baseline | Treatment | Change | p-value | Verdict |
|-----------|----------|-----------|--------|---------|---------|
| simple_variable | 194.52 ns | 195.94 ns | +0.73% | < 0.05 | Within noise |
| mixed_complexity | 606.19 ns | 599.69 ns | -1.07% | < 0.05 | Within noise |
| failures/type_mismatch | 139.45 ns | 138.37 ns | -0.77% | < 0.05 | Within noise |

**rule_matching benchmarks** (uses HashMap for rule_index):

| Benchmark | Baseline | Treatment | Change | p-value | Verdict |
|-----------|----------|-----------|--------|---------|---------|
| fibonacci_lookup/10 | 759.64 µs | 745.07 µs | -1.9% | > 0.05 | Not significant |
| fibonacci_lookup/100 | 5.54 ms | 5.63 ms | +1.7% | < 0.05 | Regression |
| fibonacci_lookup/1000 | 51.22 ms | 52.42 ms | +2.3% | < 0.05 | Regression |
| pattern_matching/simple | 81.68 µs | 79.30 µs | -2.9% | < 0.05 | Improvement |
| pattern_matching/nested | 172.25 µs | 180.80 µs | +5.0% | < 0.05 | Regression |
| worst_case_lookup/1000 | 60.54 ms | 57.74 ms | -4.6% | < 0.05 | Improvement |

**metta end-to-end benchmarks** (divan):

| Benchmark | Baseline | Treatment | Change | Verdict |
|-----------|----------|-----------|--------|---------|
| async_fib | 7.302 ms | 7.330 ms | +0.4% | Within noise |
| async_knowledge_graph | 2.759 ms | 2.701 ms | -2.1% | Marginal improvement |

### Decision: **REJECT**

Does not meet acceptance criteria:
- No consistent > 2% improvement across benchmarks
- Several statistically significant regressions detected (+2.3% to +5.0%)
- Hypothesis of 30-50% improvement not supported

### Analysis

The ahash optimization showed mixed results:

1. **Pattern matching benchmarks**: Mostly unchanged because SmartBindings uses a custom hybrid
   data structure (Empty/Single/SmallVec), not HashMap. ahash optimization only affects HashMap.

2. **Rule matching benchmarks**: Mixed results with both improvements and regressions.
   The regressions may be due to:
   - ahash's AES-NI optimization has higher setup cost for small HashMap sizes
   - The rule_index is accessed via read locks which dominate the hash time
   - String key hashing may not benefit as much as integer keys

3. **End-to-end evaluation**: Minimal impact (-2% to +0.4%), within measurement noise.

4. **Key insight**: The bottleneck is not hashing but rather:
   - Lock contention on shared structures (rule_index, bindings)
   - Memory allocation/deallocation (40% of perf profile)
   - MORK serialization and PathMap operations

The feature will be reverted from this branch. Consider alternative optimizations that address
the actual bottlenecks (lock-free data structures, arena allocation).

---

## Experiment 2: Symbol Interning (lasso)

**Branch**: `perf/cpu-opt-symbol-intern`
**Hypothesis**: String interning will reduce symbol comparison overhead by 20-40%
**Status**: PENDING

### Changes
- Add `lasso = { version = "0.7", features = ["multi-threaded"], optional = true }` to Cargo.toml
- Create `src/backend/symbol.rs` with Symbol wrapper type
- Modify rule_index key from `(String, usize)` to `(Symbol, usize)`

### Results

| Benchmark | Baseline | Treatment | Change | p-value | Cohen's d |
|-----------|----------|-----------|--------|---------|-----------|
| TBD | TBD | TBD | TBD | TBD | TBD |

### Decision: TBD

### Analysis
TBD

---

## Experiment 3: Small Map Optimization (micromap)

**Branch**: `perf/cpu-opt-small-maps`
**Hypothesis**: Using micromap for small maps (<16 keys) will improve performance by 50-70%
**Status**: PENDING

### Changes
- Add `micromap = { version = "0.0.15", optional = true }` to Cargo.toml
- Create `src/backend/adaptive_map.rs` with AdaptiveMap type
- Apply to small temporary maps in evaluation

### Results

| Benchmark | Baseline | Treatment | Change | p-value | Cohen's d |
|-----------|----------|-----------|--------|---------|-----------|
| TBD | TBD | TBD | TBD | TBD | TBD |

### Decision: TBD

### Analysis
TBD

---

## Experiment 4: POPCNT Optimization

**Branch**: `perf/cpu-opt-popcnt`
**Hypothesis**: Hardware POPCNT will improve bloom filter operations
**Status**: PENDING

### Changes
- Create `src/backend/cpu_features.rs` with popcount functions
- Add runtime detection for `portable` feature
- Integrate with bloom filter operations

### Results

| Benchmark | Baseline | Treatment | Change | p-value | Cohen's d |
|-----------|----------|-----------|--------|---------|-----------|
| TBD | TBD | TBD | TBD | TBD | TBD |

### Decision: TBD

### Analysis
TBD

---

## Statistical Methods

### Two-Sample Welch's t-test

Used for comparing means between baseline and treatment groups:

```
t = (mean1 - mean2) / sqrt(var1/n1 + var2/n2)
df = (var1/n1 + var2/n2)^2 / ((var1/n1)^2/(n1-1) + (var2/n2)^2/(n2-1))
```

### Cohen's d Effect Size

```
d = (mean1 - mean2) / pooled_std
pooled_std = sqrt(((n1-1)*var1 + (n2-1)*var2) / (n1+n2-2))
```

Interpretation:
- |d| < 0.2: Negligible
- 0.2 <= |d| < 0.5: Small
- 0.5 <= |d| < 0.8: Medium
- |d| >= 0.8: Large

---

## Conclusions

TBD - Will be populated after all experiments complete

---

## Appendix: Raw Data

### Criterion JSON Output Locations
- Baseline: `target/criterion/baseline/`
- Experiment 1: `target/criterion/ahash/`
- Experiment 2: `target/criterion/symbol-intern/`
- Experiment 3: `target/criterion/small-maps/`
- Experiment 4: `target/criterion/popcnt/`

### Perf Data Locations
- Baseline: `/tmp/perf_baseline.data`
- Per-experiment: `/tmp/perf_<experiment>.data`
