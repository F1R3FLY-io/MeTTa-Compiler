# MeTTaTron Baseline Performance Metrics

## Overview

This document establishes baseline performance metrics for MeTTaTron before implementing PathMap ACT persistence strategies. These metrics will be compared with post-implementation results to quantitatively validate improvements.

**Date**: 2025-11-22
**System**: See Hardware Specifications below
**Branch**: `dylon/main`
**Commit**: `22d5f01` - fix(build): Prevent unnecessary Tree-Sitter parser regeneration

---

## Hardware Specifications

- **CPU**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads)
- **RAM**: 252 GB DDR4-2133 ECC
- **Storage**: Samsung SSD 990 PRO 4TB NVMe
- **OS**: Linux 6.17.8-arch1-1
- **Rust**: `rustc --version` output to be added
- **Optimization**: `--release` profile (opt-level=3, LTO=true, codegen-units=1)

---

## Benchmark Suite

The benchmark suite (`benches/kb_persistence.rs`) tests various aspects of knowledge base operations:

1. **Compilation Benchmarks**: Parse + compile MeTTa source to internal representation
   - Small KB (~10K rules, ~1MB equivalent)
   - Medium KB (~100K rules, ~10MB equivalent)
   - Large KB (~1M rules, ~100MB equivalent)

2. **Memory Footprint Benchmarks**: Measure memory usage for compiled KBs

3. **Serialization/Deserialization Benchmarks**: Current file I/O performance

4. **Query Performance Benchmarks**:
   - Cold cache (first access)
   - Warm cache (repeated access)

5. **Startup Time Benchmarks**: Total time from source to ready-to-query KB

---

## Baseline Results

### Compilation Performance

| Benchmark | Median | Mean | Samples | Notes |
|-----------|--------|------|---------|-------|
| `compile_small_kb` (10K rules) | TBD | TBD | 100 | ~1MB MeTTa source |
| `compile_medium_kb` (100K rules) | TBD | TBD | 100 | ~10MB MeTTa source |
| `compile_large_kb` (1M rules) | TBD | TBD | 10 | ~100MB MeTTa source (fewer samples) |

### Memory Footprint

| Benchmark | Memory Usage | Notes |
|-----------|--------------|-------|
| `memory_footprint_small_kb` | TBD | Resident set size measurement |
| `memory_footprint_medium_kb` | TBD | Resident set size measurement |

### Serialization Performance

| Benchmark | Median | Mean | Notes |
|-----------|--------|------|-------|
| `serialize_small_kb` | TBD | TBD | Debug format + file write |
| `deserialize_small_kb` | TBD | TBD | File read only (no parser yet) |

### Query Performance

| Benchmark | Median | Mean | Notes |
|-----------|--------|------|-------|
| `query_cold_small_kb` | TBD | TBD | First access (cold cache) |
| `query_warm_small_kb` | TBD | TBD | Repeated access (warm cache) |

### Startup Time

| Benchmark | Median | Mean | Notes |
|-----------|--------|------|-------|
| `startup_time_small_kb` | TBD | TBD | Parse + compile |
| `startup_time_medium_kb` | TBD | TBD | Parse + compile |

---

## Profiling Analysis

### Bottleneck Identification

**Method**: `perf record` + analysis

#### Hotspots

TBD - Top functions by CPU time:

1. TBD
2. TBD
3. TBD

#### Page Fault Analysis

TBD - Page fault frequency and patterns

---

## Statistical Summary

### Compilation Time Scaling

| KB Size | Rules | Time | Time per Rule | Scaling Factor |
|---------|-------|------|---------------|----------------|
| Small   | 10K   | TBD  | TBD           | 1.0× (baseline) |
| Medium  | 100K  | TBD  | TBD           | TBD |
| Large   | 1M    | TBD  | TBD           | TBD |

### Expected Performance Characteristics

**Current Implementation** (interpretation-based):
- **Load time**: O(n×m) where n=KB size, m=average rule complexity
- **Memory usage**: O(n) - all rules in memory
- **Serialization**: Debug format (unoptimized)

---

## Hypothesis for PathMap ACT Integration

Based on PathMap persistence documentation analysis:

### Expected Improvements

1. **Load Time**: 100-1000× faster via O(1) memory-mapped loading
2. **Memory Usage**: 50-70% reduction via:
   - Term interning (deduplication)
   - Merkleization (structural sharing)
3. **File Size**: 70% reduction via compression + deduplication
4. **Cold vs Warm**: 100× difference eliminated via page cache pre-warming

### Metrics to Validate

- [ ] Startup time for large KBs (target: <1s for 1GB KB)
- [ ] Memory footprint reduction (target: 50%+)
- [ ] File size reduction (target: 70%+)
- [ ] Page fault frequency (expect reduction via better locality)

---

## Benchmark Execution

### Running Benchmarks

```bash
# All benchmarks (except large KB)
cargo bench --bench kb_persistence -- --skip compile_large_kb

# Specific benchmark
cargo bench --bench kb_persistence -- compile_small_kb

# With profiling
cargo bench --bench kb_persistence --profile=profiling
```

### Profiling Commands

```bash
# CPU profiling
perf record -F 99 -g cargo bench --bench kb_persistence --profile=profiling -- compile_medium_kb
perf report

# Page fault tracking
perf stat -e page-faults,minor-faults,major-faults cargo bench --bench kb_persistence -- compile_medium_kb
```

---

## Notes

- Benchmarks use `divan` framework for statistical rigor
- Each benchmark runs 100 samples (10 for large KB) for statistical significance
- CPU affinity and frequency scaling should be considered for production benchmarks
- jemalloc allocator enabled via PathMap dependency (provides 100-1000× parallel speedup)

---

## Next Steps

1. ✅ Establish baseline metrics (this document)
2. ⬜ Profile with `perf` to identify bottlenecks
3. ⬜ Implement PathMap ACT persistence
4. ⬜ Re-run benchmarks with same configuration
5. ⬜ Compare results and validate hypothesis
6. ⬜ Statistical analysis of improvements

---

**Document Status**: In Progress - Awaiting benchmark results
