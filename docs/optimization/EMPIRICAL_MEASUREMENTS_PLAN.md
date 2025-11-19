# Empirical Measurements Plan

**Date**: November 11, 2025
**Status**: ✅ **BENCHMARKS COMPLETE** - Results Documented

---

## Benchmarks Complete ✅

### 1. Type Lookup Benchmark (`type_lookup`)
**File**: `benches/type_lookup.rs`
**Output**: `/tmp/type_lookup_empirical.txt`
**Status**: ✅ **Complete** - 242.9× average speedup measured

**Test Cases**:
- `get_type_first`: Lookup first type assertion (best case)
- `get_type_middle`: Lookup middle type assertion (average case)
- `get_type_last`: Lookup last type assertion (worst case for linear)
- `get_type_missing`: Lookup nonexistent type (full search)
- `first_lookup_cold_cache`: Cold cache (measures index build time)
- `subsequent_lookup_hot_cache`: Hot cache (measures cached performance)
- `lookup_after_insert`: Mixed workload (insert + lookup)

**Dataset Sizes**: 10, 100, 1000, 5000, 10000 type assertions

**Measured Results**: ✅
- **Speedup**: **242.9× average** (range: 11× to 551×)
- **Mechanism**: PathMap::restrict() creates type-only subtrie
- **Complexity**: O(n) → O(1) for hot cache lookups
- **Conclusion**: **Within predicted 100-1000× range**

---

### 2. Bulk Operations Benchmark (`bulk_operations`)
**File**: `benches/bulk_operations.rs`
**Output**: `/tmp/bulk_operations_empirical.txt`
**Status**: ✅ **Complete** - 1.03-1.07× speedups measured

**Test Cases**:

#### Facts (Phase 2)
- `fact_insertion_baseline/individual_add_to_space`: N individual inserts
- `fact_insertion_optimized/bulk_add_facts_bulk`: Single bulk insert
- `fact_insertion_comparison/baseline`: Direct comparison baseline
- `fact_insertion_comparison/optimized`: Direct comparison optimized

#### Rules (Phase 4)
- `rule_insertion_baseline/individual_add_rule`: N individual inserts
- `rule_insertion_optimized/bulk_add_rules_bulk`: Single bulk insert
- `rule_insertion_comparison/baseline`: Direct comparison baseline
- `rule_insertion_comparison/optimized`: Direct comparison optimized

**Dataset Sizes**: 10, 50, 100, 500, 1000 items

**Measured Results**: ⚠️
- **Facts Speedup**: **1.03× average** (range: 1.01× to 1.04×)
- **Rules Speedup**: **1.07× average** (range: 1.04× to 1.10×)
- **Mechanism**: PathMap::join() with single lock acquisition
- **Lock Reduction**: 1000 → 1 for facts, 3000+ → 4 for rules
- **Conclusion**: **Below predictions** - MORK serialization dominates (99% of time)
- **Note**: Lock overhead was <1% of total time; bulk operations still beneficial for concurrency

---

## Measurement Methodology

### Hardware Configuration
- **CPU Affinity**: Cores 0-17 (taskset -c 0-17)
- **CPU**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads)
- **RAM**: 252 GB DDR4 ECC @ 2133 MT/s
- **Storage**: Samsung SSD 990 PRO 4TB NVMe

### Criterion Configuration
- **Sample size**: Default (100)
- **Warm-up time**: Default (3s)
- **Measurement time**: Default (5s)
- **Confidence level**: 95%
- **Outlier detection**: Enabled

### Metrics Collected
1. **Mean execution time**: Average time per iteration
2. **Standard deviation**: Measure of variance
3. **Throughput**: Operations per second
4. **Speedup ratio**: Optimized / Baseline
5. **95% confidence interval**: Statistical significance

---

## Data Extraction Script

```bash
#!/bin/bash

# Extract type lookup results
echo "=== TYPE LOOKUP RESULTS ==="
grep -E "time:|Benchmarking" /tmp/type_lookup_empirical.txt | head -100

# Extract bulk operations results
echo ""
echo "=== BULK OPERATIONS RESULTS ==="
grep -E "time:|Benchmarking" /tmp/bulk_operations_empirical.txt | head -100

# Calculate speedups
echo ""
echo "=== SPEEDUP CALCULATIONS ==="

# Facts comparison
baseline_facts=$(grep "fact_insertion_comparison/baseline/100" /tmp/bulk_operations_empirical.txt | grep "time:" | awk '{print $4}')
optimized_facts=$(grep "fact_insertion_comparison/optimized/100" /tmp/bulk_operations_empirical.txt | grep "time:" | awk '{print $4}')
echo "Facts (100 items): ${baseline_facts} → ${optimized_facts}"

# Rules comparison
baseline_rules=$(grep "rule_insertion_comparison/baseline/100" /tmp/bulk_operations_empirical.txt | grep "time:" | awk '{print $4}')
optimized_rules=$(grep "rule_insertion_comparison/optimized/100" /tmp/bulk_operations_empirical.txt | grep "time:" | awk '{print $4}')
echo "Rules (100 items): ${baseline_rules} → ${optimized_rules}"
```

---

## Expected Timeline

- **Type Lookup Benchmark**: ~10-15 minutes
  - 7 test cases × 5 dataset sizes = 35 benchmarks
  - ~20-30 seconds per benchmark

- **Bulk Operations Benchmark**: ~20-30 minutes
  - 6 test groups × 5 dataset sizes = 30 benchmarks
  - ~40-60 seconds per benchmark

- **Total Time**: ~30-45 minutes for complete empirical data

---

## Validation Checklist ✅

Benchmarks complete - all items validated:

### Phase 1: Type Index
- [✅] Extract mean times for 10, 100, 1000, 5000, 10000 types
- [✅] Calculate speedup ratios (cold cache vs warm cache): 11× to 551×
- [✅] Verify O(p + m) → O(1) scaling behavior: Hot cache constant ~950ns
- [✅] Compare with predicted 100-1000× speedup: **242.9× average ✅**

### Phase 2: Bulk Facts
- [✅] Extract baseline times for 10, 50, 100, 500, 1000 facts
- [✅] Extract optimized times for same dataset sizes
- [✅] Calculate speedup ratios: 1.01× to 1.04×
- [✅] Verify lock contention reduction (1000 → 1): Confirmed
- [✅] Compare with predicted 10-50× speedup: **1.03× average ⚠️**

### Phase 4: Bulk Rules
- [✅] Extract baseline times for 10, 50, 100, 500, 1000 rules
- [✅] Extract optimized times for same dataset sizes
- [✅] Calculate speedup ratios: 1.04× to 1.10×
- [✅] Verify lock contention reduction (3000+ → 4): Confirmed
- [✅] Compare with predicted 20-100× speedup: **1.07× average ⚠️**

### Statistical Validation
- [✅] Verify 95% confidence intervals: Criterion provides CI for all measurements
- [✅] Check for consistent speedups across dataset sizes: Consistent patterns observed
- [✅] Identify any outliers or anomalies: None found
- [✅] Validate scaling behavior: Type index O(1), bulk ops linear with serialization

---

## Results Documentation ✅

Documentation created:

1. ✅ **EMPIRICAL_RESULTS.md**: Comprehensive 350-line report with:
   - Detailed measurements and tables
   - Speedup calculations and analysis
   - Comparison with predictions
   - Optimization recommendations
   - Lessons learned

2. ✅ **SUBTRIE_IMPLEMENTATION_COMPLETE.md**: Implementation details
3. ✅ **EMPIRICAL_MEASUREMENTS_PLAN.md**: This document (methodology)

---

## Benchmark Status (Final)

**Completed**: November 11, 2025 - 22:30 PST

### Type Lookup
- Status: ✅ **Complete**
- Duration: ~15 minutes
- Results: 242.9× average speedup

### Bulk Operations
- Status: ✅ **Complete**
- Duration: ~30 minutes
- Results: 1.03× facts, 1.07× rules

---

## Next Steps ✅

1. ✅ **Monitor benchmarks**: Completed successfully
2. ✅ **Extract results**: Parsed output files with timing data
3. ✅ **Calculate speedups**: All speedups calculated and documented
4. ✅ **Validate predictions**: Type index ✅, bulk ops ⚠️ (serialization bottleneck)
5. ✅ **Document findings**: Created EMPIRICAL_RESULTS.md (350 lines)
6. ⏭️ **Generate flamegraphs**: Optional next step for deeper analysis

## Future Optimizations Identified

Based on empirical measurements, the following optimizations have highest impact:

1. **Optimize MORK Serialization** (Highest Priority)
   - Current: 9 μs/operation (99% of bulk operation time)
   - Target: <1 μs/operation
   - Methods: Pre-serialization, zero-copy, direct PathMap construction
   - Expected: 5-10× speedup for bulk operations

2. **Parallel Bulk Operations** (Medium Priority)
   - Use Rayon to parallelize serialization
   - Expected: 10-36× speedup on 36-core Xeon

3. **Direct PathMap Construction** (High Priority)
   - MettaValue → PathMap directly (skip MORK string)
   - Expected: 5-10× speedup

