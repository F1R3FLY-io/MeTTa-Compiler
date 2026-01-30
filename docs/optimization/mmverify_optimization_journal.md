# mmverify Optimization Journal

## Problem Statement

The mmverify benchmark has regressed from ~80s (best historical) to >251s (terminated early) after recent changes on the `pr30/improved-multiplicity-tracking` branch. This is a >3x regression.

## Environment Configuration

- **Branch**: `pr30/improved-multiplicity-tracking`
- **Starting Commit**: `84cea91659913b43ba1199f8115840a0a4c1e838`
- **CPU**: 18 cores (taskset 0-17)
- **METTATRON_NUM_THREADS**: 18
- **Statistical threshold**: p < 0.05 for accepting/rejecting optimizations
- **Minimum runs**: 5 per experiment

---

## Phase 1: Baseline Profiling

### Step 1.1: Build Configuration

**Date**: 2026-01-30
**Command**: `cargo build --profile=release-with-debug`
**Status**: Complete

### Step 1.2: Baseline Measurements

Baseline timing exceeds 250s - run terminated early to proceed with profiling.
**Baseline**: >250s (confirmed regression from ~80s historical best)

### Step 1.3: perf Profile Analysis (90s sample, 89,355 samples)

**Top Hotspots**:

| Function | % | Category |
|----------|---|----------|
| `mork_expr_to_metta_value` | 11.83% | MORK→MeTTa conversion |
| `k_path_default_internal` | 10.79% | PathMap zipper |
| `child_mask` | 7.51% | PathMap zipper |
| `coreferential_transition` | 6.76% | MORK query_multi |
| `to_next_sibling_byte` | 5.45% | PathMap zipper |
| `regularize` | 3.73% | PathMap zipper |
| `ascend_byte` | 3.05% | PathMap zipper |
| `_rjem_malloc` | 2.82% | Memory allocation |
| `MettaValue::drop` | 2.44% | Memory deallocation |
| `_rjem_sdallocx` | 2.18% | Memory deallocation |
| `descend_to_byte` | 2.11% | PathMap zipper |

**Analysis**:
- PathMap zipper operations total ~33% (iteration overhead)
- `mork_expr_to_metta_value` at 11.83% (conversion on every atom)
- Memory allocation/deallocation ~8% (jemalloc + MettaValue::drop)

**Root Cause Identified**:
The `match_space()` function iterates ALL atoms (O(n)) and converts each with `mork_expr_to_metta_value()`,
even when only k << n atoms match the pattern. MORK provides `query_multi` for O(k) native matching
but `match_space()` doesn't use it.

**Hot Path**: `match &kb pattern template` → `env.match_space()` → iterate all atoms → convert each → pattern_match()

### Step 1.4: Optimization Strategy

**Primary Optimization**: Replace O(n) iteration with O(k) MORK query_multi in `match_space()`
- Currently: Iterate all atoms → Convert each → Pattern match → Keep matches
- Proposed: Use MORK query_multi → Get bindings directly → Apply to template

**Secondary Optimizations** (if needed):
- Reduce string allocations in `mork_expr_to_metta_value` (VARNAMES.to_string() overhead)
- Content-based caching for converted MettaValues

---

## Phase 2: Experiments

### Experiment 1: [TBD based on profiling]

**Hypothesis**: TBD
**Predicted improvement**: TBD

**Before Changes**:
- Mean: TBD
- Std Dev: TBD

**After Changes**:
- Mean: TBD
- Std Dev: TBD

**Statistical Analysis**:
- t-statistic: TBD
- p-value: TBD
- Decision: TBD

---

## Summary

| Experiment | Predicted | Actual | p-value | Decision |
|------------|-----------|--------|---------|----------|
| Baseline   | N/A       | TBD    | N/A     | N/A      |

---

## Appendix: Hardware Specifications

See `/home/dylon/.claude/hardware-specifications.md`
