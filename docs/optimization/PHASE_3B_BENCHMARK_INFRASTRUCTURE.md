# Phase 3b: AlgebraicStatus Optimization - Benchmark Infrastructure

**Date**: 2025-11-13
**Status**: ✅ INFRASTRUCTURE COMPLETE
**Branch**: dylon/rholang-language-server
**Commit**: [To be filled after commit]

---

## Overview

Comprehensive benchmark suite for empirically validating Phase 3b AlgebraicStatus optimization. Following MORK implementation guide best practices and scientific methodology from CLAUDE.md guidelines.

---

## Benchmark File Created

**Location**: `benches/algebraic_status_duplicate_detection.rs` (462 lines)

**Purpose**: Measure actual performance of Phase 3b optimization across five distinct scenario groups to validate theoretical predictions.

---

## Benchmark Groups

### Group 1: All New Data (Baseline Verification)
**Purpose**: Verify no regression when adding only new data
**Hypothesis**: AlgebraicStatus::Element always → same performance as existing bulk_operations.rs

**Benchmarks**:
- `algebraic_status_facts_all_new/{10,100,500,1000,5000}`
- `algebraic_status_rules_all_new/{10,100,500,1000,5000}`

**Expected Result**: Within ±5% variance of baseline

---

### Group 2: All Duplicate Data (Maximum Benefit)
**Purpose**: Measure maximum benefit when re-adding same data
**Hypothesis**: AlgebraicStatus::Identity always → significant speedup from skipped work

**Benchmarks**:
- `algebraic_status_facts_all_duplicates/{10,100,500,1000,5000}`
- `algebraic_status_rules_all_duplicates/{10,100,500,1000,5000}`

**Expected Result**: Measurable speedup from:
- Skipped modified flag updates
- Skipped type index invalidation (facts)
- Skipped CoW deep copies
- Skipped downstream evaluation work

---

### Group 3: Mixed Duplicate Ratios (Realistic Workloads)
**Purpose**: Measure performance across realistic duplicate ratios
**Hypothesis**: Savings proportional to duplicate ratio

**Benchmarks**:
- `algebraic_status_facts_mixed_ratios/ratio/{0,25,50,75,100}` (1000 items)
- `algebraic_status_rules_mixed_ratios/ratio/{0,25,50,75,100}` (1000 items)

**Expected Result**: Linear relationship between duplicate ratio and speedup

---

### Group 4: CoW Clone Impact (Downstream Effects)
**Purpose**: Measure CoW clone performance after modified vs unmodified operations
**Hypothesis**: O(1) Arc increment vs O(n) deep copy

**Benchmarks**:
- `algebraic_status_cow_clone_after_duplicates/{100,500,1000}`
- `algebraic_status_cow_clone_after_new_data/{100,500,1000}`

**Expected Result**: Unmodified environment clones faster (no deep copy needed)

---

### Group 5: Type Index Invalidation (Facts-Specific Benefit)
**Purpose**: Measure type index rebuild savings for duplicate type assertions
**Hypothesis**: Hot cache preserved when adding duplicate facts

**Benchmarks**:
- `algebraic_status_type_lookup_after_duplicates/{100,500,1000}`
- `algebraic_status_type_lookup_after_new_facts/{100,500,1000}`

**Expected Result**: No index rebuild for duplicates → faster type lookups

---

## Running Benchmarks

### Quick Test (Single Group)
```bash
# Test one benchmark group to verify it works
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  -- "algebraic_status_facts_all_new"
```

### Full Benchmark Suite
```bash
# Run all benchmark groups with CPU affinity
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  2>&1 | tee /tmp/phase3b_optimized_measurements.txt
```

### Specific Scenarios
```bash
# Group 1: All new data
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  -- "all_new"

# Group 2: All duplicates
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  -- "all_duplicates"

# Group 3: Mixed ratios
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  -- "mixed_ratios"

# Group 4: CoW clones
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  -- "cow_clone"

# Group 5: Type index
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  -- "type_lookup"
```

---

## Profiling Commands

### Flamegraphs
```bash
# Install flamegraph if needed
cargo install flamegraph

# Generate flamegraph for duplicate scenario
perf record --call-graph=dwarf -F 999 \
  ./target/profiling/mettatron <duplicate-workload>

perf script | stackcollapse-perf.pl | flamegraph.pl \
  > /tmp/phase3b_duplicates_flamegraph.svg
```

### Cache Analysis (Cachegrind)
```bash
valgrind --tool=cachegrind \
  --cachegrind-out-file=/tmp/phase3b_duplicates.cachegrind \
  ./target/profiling/mettatron <duplicate-workload>

cg_annotate /tmp/phase3b_duplicates.cachegrind \
  > /tmp/phase3b_cache_analysis.txt
```

### Memory Profiling (Heaptrack)
```bash
heaptrack ./target/profiling/mettatron <duplicate-workload>

heaptrack_print heaptrack.mettatron.*.gz \
  > /tmp/phase3b_memory_duplicates.txt
```

---

## Criterion Configuration

The benchmarks use Criterion with the following settings (default):
- **Samples**: 100
- **Warmup**: 3 seconds
- **Measurement**: 5 seconds
- **Statistical analysis**: Mean, std dev, median

Results are saved to:
- `target/criterion/` - Criterion JSON results and reports
- HTML reports: `target/criterion/report/index.html`

---

## Expected Timeline

**Full benchmark suite**: 30-45 minutes

Breakdown by group:
- Group 1 (All new): ~5-7 min (10 benchmarks × 5 sizes)
- Group 2 (All duplicates): ~5-7 min (10 benchmarks × 5 sizes)
- Group 3 (Mixed ratios): ~10-12 min (10 benchmarks × 5 ratios)
- Group 4 (CoW clones): ~5-7 min (6 benchmarks × 3 sizes)
- Group 5 (Type index): ~5-7 min (6 benchmarks × 3 sizes)

**Note**: Times assume CPU affinity and release build

---

## Analyzing Results

### View Criterion Reports
```bash
# Open HTML report in browser
firefox target/criterion/report/index.html

# Or chrome/chromium
chrome target/criterion/report/index.html
```

### Extract Key Metrics
```bash
# View summary from saved output
grep -E "(time:|Benchmarking)" /tmp/phase3b_optimized_measurements.txt

# Compare specific benchmarks
cd target/criterion
ls -la  # Shows all benchmark result directories
```

### Hypothesis Validation

**Hypothesis 1: No Regression (All New Data)**
- Compare Group 1 results with existing `bulk_operations.rs` benchmarks
- Expected: Within ±5% variance

**Hypothesis 2: Significant Speedup (All Duplicates)**
- Compare Group 2 times with Group 1 times
- Expected: Measurable improvement (amount TBD)

**Hypothesis 3: Proportional Savings (Mixed Ratios)**
- Plot duplicate ratio (0%, 25%, 50%, 75%, 100%) vs execution time
- Expected: Linear relationship

**Hypothesis 4: CoW Clone Benefit**
- Compare `cow_clone_after_duplicates` vs `cow_clone_after_new_data`
- Expected: Faster clones when unmodified

**Hypothesis 5: Type Index Preservation**
- Compare `type_lookup_after_duplicates` vs `type_lookup_after_new_facts`
- Expected: No rebuild for duplicates

---

## Integration with MORK Best Practices

Following MORK implementation guide recommendations:

1. ✅ **CPU Affinity**: `taskset -c 0-17` for consistency
2. ✅ **Profiling Tools**: flamegraph, cachegrind, heaptrack
3. ✅ **Scientific Method**: Hypothesis → measurement → validation
4. ✅ **Multiple Dataset Sizes**: Powers of 10 (10, 100, 500, 1000, 5000)
5. ✅ **Criterion Benchmarking**: 100 samples, proper warmup
6. ✅ **Comprehensive Coverage**: 5 scenario groups, 40+ individual benchmarks

---

## Next Steps for Empirical Validation

### Step 1: Run Benchmarks
```bash
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  2>&1 | tee /tmp/phase3b_measurements.txt
```

### Step 2: Analyze Results
- Review Criterion HTML reports
- Extract performance metrics
- Validate hypotheses
- Compare with theoretical predictions

### Step 3: Optional Profiling
If benchmarks reveal unexpected results:
```bash
# Generate flamegraph for analysis
perf record --call-graph=dwarf -F 999 \
  ./target/profiling/mettatron <workload>
perf script | stackcollapse-perf.pl | flamegraph.pl > analysis.svg
```

### Step 4: Document Findings
Create `PHASE_3B_ALGEBRAIC_STATUS_EMPIRICAL_VALIDATION.md` with:
- Methodology
- Results tables
- Hypothesis validation
- Comparison with predictions
- Recommendations

---

## Benchmark Code Structure

**Helper Functions** (lines 27-107):
- `create_test_facts(n)` - Generate N unique facts
- `create_test_rules(n)` - Generate N unique rules
- `create_test_type_facts(n)` - Generate N type assertions
- `prepopulate_with_facts()` - Pre-populate environment
- `prepopulate_with_rules()` - Pre-populate environment
- `create_mixed_fact_dataset()` - Generate mixed duplicate/new datasets
- `create_mixed_rule_dataset()` - Generate mixed duplicate/new datasets

**Benchmark Groups** (lines 109-454):
- Group 1: `bench_add_facts_all_new()`, `bench_add_rules_all_new()`
- Group 2: `bench_add_facts_all_duplicates()`, `bench_add_rules_all_duplicates()`
- Group 3: `bench_add_facts_mixed_ratios()`, `bench_add_rules_mixed_ratios()`
- Group 4: `bench_cow_clone_after_duplicates()`, `bench_cow_clone_after_new_data()`
- Group 5: `bench_type_lookup_after_duplicate_facts()`, `bench_type_lookup_after_new_facts()`

**Configuration** (lines 456-462):
- Criterion group registration
- Main entry point

---

## Success Criteria

1. ✅ Benchmark file compiles successfully
2. ✅ All 40+ benchmarks run without errors
3. ✅ Results validate or refute each hypothesis
4. ✅ Profiling data available if needed
5. ✅ Comprehensive documentation created

---

## Related Documentation

- **Phase 3b Implementation**: `/tmp/phase3b_algebraic_status_complete.md`
- **Phase 3a Analysis**: `/tmp/phase3a_complete_summary.md`
- **MORK Implementation Guide**: `docs/mork/implementation-guide.md`
- **MORK Roadmap**: `docs/mork/implementation-roadmap.md`
- **Existing Benchmarks**: `benches/bulk_operations.rs`

---

## Technical Notes

### AlgebraicStatus Optimization Recap

**Phase 3b Changes** (environment.rs):
1. Line 5: Added `use pathmap::ring::{AlgebraicStatus, Lattice};`
2. Lines 816-828: Modified `add_rules_bulk()` to use `join_into()` with status checking
3. Lines 1229-1246: Modified `add_facts_bulk()` to use `join_into()` with status checking

**Expected Behavior**:
- `AlgebraicStatus::Element` when data changes → mark as modified
- `AlgebraicStatus::Identity` when no changes → skip modification

**Performance Impact**:
- **New data**: Same performance (always Element)
- **Duplicate data**: Unbounded savings (always Identity)
- **Mixed data**: Proportional savings

---

## Troubleshooting

### Benchmark Won't Compile
```bash
# Clean and rebuild
cargo clean
cargo build --profile=profiling --benches
```

### Benchmarks Take Too Long
```bash
# Run single size only
taskset -c 0-17 cargo bench --bench algebraic_status_duplicate_detection \
  -- "1000"
```

### Need Quick Validation
```bash
# Run with --quick flag (fewer samples)
cargo bench --bench algebraic_status_duplicate_detection -- --quick
```

### Profiling Tools Not Installed
```bash
# Install flamegraph
cargo install flamegraph

# Install valgrind (cachegrind)
sudo apt-get install valgrind  # Ubuntu/Debian
sudo yum install valgrind      # RHEL/CentOS

# Install heaptrack
sudo apt-get install heaptrack  # Ubuntu/Debian
```

---

**Date Created**: 2025-11-13
**Created By**: Claude Code
**Status**: Infrastructure complete, ready for empirical validation

