# Branch Comparison Benchmark Guide

**Status**: Active Infrastructure
**Created**: 2025-11-14
**Purpose**: Automated performance comparison between git branches

## Overview

The branch comparison benchmark system provides automated, reproducible performance testing to validate optimizations across git branches. It measures both **runtime performance** and **space efficiency** across 8 major performance dimensions.

## Quick Start

### Running a Comparison

```bash
# Compare current branch with main
./scripts/compare_branches.sh

# Compare with a different branch
./scripts/compare_branches.sh develop

# Results are saved to benchmark_results_TIMESTAMP/
```

### Reading the Report

```bash
# View the comparison report
cat benchmark_results_*/comparison_report.md

# View raw results
cat benchmark_results_*/current_branch.txt
cat benchmark_results_*/main_branch.txt
```

## Benchmark Dimensions

The benchmark suite covers 8 major performance dimensions:

### 1. Prefix Fast Path (Phase 3a Validation)

**What it tests**: `has_sexpr_fact()` lookups with ground (non-variable) patterns

**Expected results**:
- **Current branch**: ~1,024× faster than main
- **Baseline**: 100-10,000 facts
- **Optimization**: Prefix-based trie fast path

**Example output**:
```
prefix_fast_path/has_sexpr_fact_ground/10000
  Main:     123.45 µs
  Current:    0.12 µs
  Speedup: ✅ 1024.00× faster
```

**Significance**: This validates the Phase 3a optimization documented in `docs/optimization/PHASE_3A_PREFIX_FAST_PATH.md`

### 2. Bulk Insertion (Phase 5 Validation)

**What it tests**: Bulk fact and rule insertion operations

**Expected results**:
- **Current branch**: ~2.0× faster than main
- **Baseline**: 10-1,000 items
- **Optimization**: Batch processing with reduced overhead

**Example output**:
```
bulk_insertion/add_facts_bulk/1000
  Main:      5.23 ms
  Current:   2.61 ms
  Speedup: ✅ 2.00× faster
```

**Significance**: Validates Phase 5 bulk insertion optimization

### 3. CoW Clone (Copy-on-Write Validation)

**What it tests**: Environment cloning cost

**Expected results**:
- **Current branch**: ~100× faster clones
- **Main branch**: O(n) cloning cost
- **Current branch**: O(1) constant time (~50ns)
- **Baseline**: 0-1,000 rules in environment

**Example output**:
```
cow_clone/env_clone/1000
  Main:      50.12 µs
  Current:   0.05 µs
  Speedup: ✅ 100.00× faster
```

**Significance**: Critical for recursive evaluation and pattern matching performance

### 4. Pattern Matching

**What it tests**: Pattern matching complexity scaling

**Scenarios**:
- Variable count: 1-50 variables per pattern
- Nesting depth: 1-10 levels deep

**Expected results**:
- **Similar performance** between branches (no optimization in this area)
- Validates baseline pattern matching performance

**Example output**:
```
pattern_matching/var_count/50
  Main:      12.34 µs
  Current:   12.45 µs
  Speedup: ➖ Similar
```

**Significance**: Ensures optimizations didn't regress pattern matching

### 5. Rule Matching

**What it tests**: Rule lookup performance with varying rule set sizes

**Expected results**:
- **Better performance** on current branch for large rule sets
- **Benefit from**: Prefix-based optimization
- **Baseline**: 10-1,000 rules

**Example output**:
```
rule_matching/rule_lookup/1000
  Main:      45.67 µs
  Current:   38.92 µs
  Speedup: ✅ 1.17× faster
```

**Significance**: Validates that prefix optimization helps rule-heavy programs

### 6. Type Lookup

**What it tests**: Type system performance (`get_type()` operations)

**Expected results**:
- **Current branch**: ~1.1× faster
- **Baseline**: 10-10,000 type assertions

**Example output**:
```
type_lookup/get_type/10000
  Main:      8.92 µs
  Current:   8.11 µs
  Speedup: ✅ 1.10× faster
```

**Significance**: Ensures type system benefits from overall optimizations

### 7. Evaluation (Rayon Removal Validation)

**What it tests**: Full expression evaluation (Phase 3c validation)

**Scenarios**:
- Simple arithmetic: `(+ 40 2)`
- Nested arithmetic: Depths 3-7

**Expected results**:
- **Similar or slightly better** on current branch
- **No parallel overhead** (Rayon removed)
- **Sequential evaluation**: Faster for MeTTa's fast operations

**Example output**:
```
evaluation/nested_arithmetic/7
  Main:      15.67 µs
  Current:   15.23 µs
  Speedup: ➖ Similar (no regression)
```

**Significance**: Validates Phase 3c decision to remove Rayon (documented in `docs/optimization/PHASE_3C_FINAL_RESULTS.md`)

### 8. Scalability

**What it tests**: Performance scaling with dataset size

**Scenarios**:
- Environment construction: 100-10,000 facts
- Lookup at scale: Search in large environments

**Expected results**:
- **Better scaling** on current branch
- **Logarithmic improvement** from trie optimizations

**Example output**:
```
scalability/lookup_at_scale/10000
  Main:      89.45 µs
  Current:    0.09 µs
  Speedup: ✅ 994.00× faster
```

**Significance**: Validates that optimizations scale well with data size

## Interpreting Results

### Speedup Indicators

The comparison report uses emoji indicators:

- ✅ **Green Check** (>10% faster): Improvement confirmed
- ❌ **Red X** (>10% slower): Performance regression detected
- ➖ **Dash** (±10%): Similar performance (within noise)

### Summary Statistics

At the end of each report:

```markdown
## Summary

- **Total benchmarks**: 50
- **Improvements** (>10% faster): 25 ✅
- **Regressions** (>10% slower): 0 ❌
- **Similar** (±10%): 25 ➖

🎉 **Success**: Performance improvements detected!
```

### Expected Summary for Current Branch

Based on implemented optimizations:

- **Improvements**: ~30-40 benchmarks (Phase 3a, 5, CoW)
- **Regressions**: 0 (all optimizations validated)
- **Similar**: ~10-20 benchmarks (baseline unchanged)

## Benchmark Configuration

### CPU Affinity

All benchmarks run with CPU affinity to ensure consistent results:

```bash
taskset -c 0-17 cargo bench --bench branch_comparison
```

**System**: Intel Xeon E5-2699 v3 (18 cores, 36 threads with HT)
**Cores used**: 0-17 (first 18 physical cores)

### Criterion Settings

- **Warmup**: Automatic (Criterion default)
- **Measurement**: Statistical sampling with confidence intervals
- **Output format**: `time: [lower_bound median upper_bound]`
- **HTML reports**: Generated in `target/criterion/`

### Build Profile

Benchmarks use the `bench` profile from `Cargo.toml`:

```toml
[profile.bench]
inherits = "release"
strip = false  # Keep debug symbols for profiling
debug = true   # Enable line-level profiling
```

## Workflow

### Automated Comparison Process

The `compare_branches.sh` script automates the following workflow:

1. **Benchmark current branch**
   ```bash
   taskset -c 0-17 cargo bench --bench branch_comparison
   ```

2. **Stash uncommitted changes** (if any)
   ```bash
   git stash push -m "Temporary stash for benchmark comparison"
   ```

3. **Switch to main branch**
   ```bash
   git checkout main
   cargo clean
   ```

4. **Benchmark main branch**
   ```bash
   taskset -c 0-17 cargo bench --bench branch_comparison
   ```

5. **Return to original branch**
   ```bash
   git checkout original_branch
   git stash pop  # Restore changes if any
   ```

6. **Generate comparison report**
   ```bash
   python3 scripts/analyze_benchmark_results.py \
       main_branch.txt \
       current_branch.txt \
       > comparison_report.md
   ```

### Manual Testing

To run benchmarks manually:

```bash
# Build benchmarks
cargo build --benches

# Run all branch_comparison benchmarks
cargo bench --bench branch_comparison

# Run specific benchmark group
cargo bench --bench branch_comparison -- prefix_fast_path

# Quick test (fewer iterations)
cargo bench --bench branch_comparison -- --quick

# View HTML reports
firefox target/criterion/report/index.html
```

## Troubleshooting

### Benchmark Compilation Errors

**Problem**: `cargo build --benches` fails

**Solution**:
```bash
# Check for missing dependencies
cargo check --benches

# Clean and rebuild
cargo clean
cargo build --benches
```

### Inconsistent Results

**Problem**: Results vary significantly between runs

**Possible causes**:
1. **CPU frequency scaling**: Ensure CPU is at max frequency
2. **Background processes**: Close unnecessary applications
3. **Thermal throttling**: Check CPU temperature
4. **NUMA effects**: Use `taskset` for CPU affinity

**Solution**:
```bash
# Check CPU frequency
cat /proc/cpuinfo | grep MHz

# Monitor during benchmark
watch -n 1 'cat /proc/cpuinfo | grep MHz'

# Set CPU governor to performance (requires root)
sudo cpupower frequency-set -g performance
```

### Python Analysis Script Errors

**Problem**: `analyze_benchmark_results.py` fails to parse output

**Solution**:
```bash
# Check Criterion output format
grep "time:" benchmark_results_*/main_branch.txt | head -5

# Test Python script directly
python3 scripts/analyze_benchmark_results.py \
    benchmark_results_*/main_branch.txt \
    benchmark_results_*/current_branch.txt
```

### Git Workflow Issues

**Problem**: Script fails to switch branches or restore state

**Solution**:
```bash
# Check for uncommitted changes
git status

# Manually stash if needed
git stash push -m "Manual stash before comparison"

# Restore original branch
git checkout your_branch_name

# Restore stashed changes
git stash list
git stash pop
```

## Files and Locations

### Source Files

- **Benchmark suite**: `benches/branch_comparison.rs` (467 lines)
- **Automation script**: `scripts/compare_branches.sh` (197 lines)
- **Analysis tool**: `scripts/analyze_benchmark_results.py` (179 lines)
- **Cargo configuration**: `Cargo.toml` (benchmark entry at lines 54-55)

### Output Files

Generated in `benchmark_results_TIMESTAMP/`:

- `main_branch.txt` - Raw Criterion output from main branch
- `current_branch.txt` - Raw Criterion output from current branch
- `comparison_report.md` - Markdown comparison report with speedups

### Criterion HTML Reports

Generated in `target/criterion/`:

- `branch_comparison/*/report/index.html` - Individual benchmark reports
- `report/index.html` - Summary report

## Related Documentation

### Optimization Documentation

- **Phase 3a**: `docs/optimization/PHASE_3A_PREFIX_FAST_PATH.md` - Prefix-based fast path (1,024× speedup)
- **Phase 3c**: `docs/optimization/PHASE_3C_FINAL_RESULTS.md` - Rayon removal justification
- **Phase 5**: `docs/optimization/PHASE_5_BULK_INSERTION.md` - Bulk insertion optimization (2.0× speedup)
- **CoW Environment**: `docs/optimization/COW_ENVIRONMENT.md` - Copy-on-Write implementation
- **Summary**: `docs/optimization/PERFORMANCE_OPTIMIZATION_SUMMARY.md` - Overall optimization strategy

### Benchmark Documentation

- **Pattern Matching**: `docs/benchmarks/pattern_matching_optimization/FINAL_REPORT.md`
- **This Guide**: `docs/benchmarks/BRANCH_COMPARISON_GUIDE.md`

## Best Practices

### Before Running Comparisons

1. **Ensure clean state**: Commit or stash all changes
2. **Close applications**: Minimize background processes
3. **Check CPU frequency**: Ensure max performance mode
4. **Verify main branch**: Ensure main is up-to-date
   ```bash
   git fetch origin
   git checkout main
   git pull origin main
   git checkout your_branch
   ```

### Interpreting Results

1. **Look for expected improvements**: Phase 3a, 5, CoW should show significant speedups
2. **Check for regressions**: Any ❌ marks indicate unexpected slowdowns
3. **Consider noise**: Small differences (±10%) may be measurement variance
4. **Validate at scale**: Check scalability benchmarks for real-world impact

### Documenting Results

When documenting benchmark results:

1. **Include timestamp**: `benchmark_results_TIMESTAMP/`
2. **Record system state**: CPU frequency, background processes
3. **Note git commits**: Include commit hashes for both branches
4. **Capture summary**: Copy the summary statistics
5. **Highlight key findings**: Document significant speedups or regressions

### Example Documentation Template

```markdown
## Benchmark Results: Branch XYZ vs Main

**Date**: 2025-11-14
**Current Branch**: dylon/rholang-language-server (commit: abc1234)
**Main Branch**: main (commit: def5678)
**System**: Intel Xeon E5-2699 v3 @ 2.30GHz (18 cores)

### Summary
- Total benchmarks: 50
- Improvements: 35 ✅
- Regressions: 0 ❌
- Similar: 15 ➖

### Key Findings
1. Prefix fast path: 1,024× speedup confirmed
2. Bulk insertion: 2.0× speedup confirmed
3. CoW environment: 100× faster clones confirmed
4. No regressions detected

### Full Report
See: benchmark_results_20251114_143052/comparison_report.md
```

## Future Enhancements

### Planned Improvements

1. **Automated CI integration**: Run comparisons on every PR
2. **Historical tracking**: Track performance over time
3. **Regression detection**: Automated alerts for performance regressions
4. **Profiling integration**: Automatic flamegraph generation
5. **Memory profiling**: Add heap profiling to benchmarks

### Extension Points

To add new benchmark dimensions:

1. **Add benchmark function** to `benches/branch_comparison.rs`:
   ```rust
   fn bench_new_dimension(c: &mut Criterion) {
       let mut group = c.benchmark_group("new_dimension");
       // ... benchmark code ...
       group.finish();
   }
   ```

2. **Register in criterion_group**:
   ```rust
   criterion_group!(
       benches,
       bench_prefix_fast_path,
       bench_new_dimension,  // Add here
       // ... other benchmarks ...
   );
   ```

3. **Update analysis script** (`scripts/analyze_benchmark_results.py`):
   ```python
   categories = {
       'new_dimension': [],  # Add category
       # ... other categories ...
   }
   ```

4. **Document in this guide**: Add section explaining the new dimension

## References

### External Documentation

- **Criterion.rs**: https://bheisler.github.io/criterion.rs/book/
- **Rust Benchmarking**: https://doc.rust-lang.org/cargo/commands/cargo-bench.html
- **CPU Affinity**: `man taskset`

### Internal Documentation

- **Threading Model**: `docs/THREADING_MODEL.md`
- **Architecture**: `.claude/CLAUDE.md`
- **Optimization Strategy**: `docs/optimization/README.md`

---

**Last Updated**: 2025-11-14
**Maintained By**: MeTTaTron Development Team
**Status**: Active
