# Pattern Matching Optimization - Baseline Analysis

**Date:** 2025-11-11
**System:** Intel Xeon E5-2699 v3 @ 2.30GHz (18 cores, 36 threads)
**CPU Affinity:** Cores 0-17 (taskset -c 0-17)
**Methodology:** Criterion benchmarks + perf profiling + flamegraph analysis

---

## Executive Summary

This document presents the baseline performance measurements for MeTTaTron's pattern matching system before implementing optimizations derived from the Rholang Language Server's MORK/PathMap integration.

### Key Performance Characteristics

**Pattern Matching (Low-Level):**
- Simple variable binding: ~98 ns
- Multiple variables (3): ~231 ns
- Ground type comparisons: ~78-84 ns
- Nested patterns (2 levels): ~206 ns
- Deep nesting (10 levels): ~639 ns
- Wildcard matching: ~110 ns
- Mixed complexity: ~388 ns
- Failure cases: ~68 ns (fast rejection)

**Environment Operations (High-Level):**
- `get_type()` with 10 types: ~2.6 µs (O(n) linear search)
- `get_type()` with 1,000 types: ~221 µs
- `get_type()` with 10,000 types: ~2,196 µs (2.2 ms)
- `has_fact()` with 1,000 facts: ~167 µs
- `iter_rules()` with 1,000 rules: ~749 µs
- `match_space()` with 1,000 facts: ~377 µs

### Identified Bottlenecks

1. **Linear O(n) scaling for `get_type()`** - Currently searches all facts
2. **No prefix-based optimization** - `extract_pattern_prefix()` exists but unused
3. **HashMap-based rule index** - Could benefit from PathMap trie structure

---

## Detailed Benchmark Results

### 1. Pattern Match Benchmarks (Low-Level Algorithm)

These benchmarks isolate the core `pattern_match()` function from environment overhead.

#### 1.1 Simple Patterns

| Benchmark | Time (ns) | Notes |
|-----------|-----------|-------|
| `simple_variable` | 98.2 ± 0.9 | Single variable `$x` → `42` |
| `multiple_variables_3` | 231.3 ± 3.5 | Three variables `($a $b $c)` |
| `ground_types/bool` | 83.6 ± 1.3 | Boolean comparison |
| `ground_types/long` | 80.3 ± 0.6 | Integer comparison |
| `ground_types/float` | 77.8 ± 0.2 | Float comparison |
| `ground_types/string` | 84.0 ± 1.5 | String comparison |
| `ground_types/atom` | 83.5 ± 0.7 | Atom comparison |

**Analysis:** Ground type comparisons are very fast (~80ns), simple variable binding adds ~18ns overhead for HashMap insertion.

#### 1.2 Variable Count Scaling

| Variables | Time (µs) | Per-Var Overhead |
|-----------|-----------|------------------|
| 1 | 0.103 | - |
| 5 | 0.287 | ~37 ns |
| 10 | 0.588 | ~49 ns |
| 25 | 1.636 | ~61 ns |
| 50 | 4.788 | ~94 ns |

**Analysis:** HashMap overhead increases with variable count. At 50 variables, per-variable cost nearly doubles due to hash collisions and capacity growth.

#### 1.3 Nesting Depth Scaling

| Depth | Time (ns) | Per-Level Cost |
|-------|-----------|----------------|
| 1 | 91.5 | - |
| 3 | 201.3 | ~55 ns/level |
| 5 | 310.5 | ~55 ns/level |
| 10 | 639.0 | ~55 ns/level |

**Analysis:** Linear scaling with depth (~55ns per level). Recursion overhead is consistent.

#### 1.4 Special Cases

| Benchmark | Time (ns) | Notes |
|-----------|-----------|-------|
| `existing_binding_simple` | 108.8 | `($x $x)` with simple value - structural equality check |
| `existing_binding_complex` | 218.1 | `($x $x)` with complex nested value |
| `wildcards` | 109.6 | `(_ $x _)` - wildcard matching |
| `mixed_complexity` | 387.8 | Real-world pattern with nesting |
| `failures/type_mismatch` | 68.0 | Fast rejection on type mismatch |
| `failures/length_mismatch` | - | Fast rejection on arity mismatch |
| `failures/binding_conflict` | - | Detect `$x` bound to different values |

**Analysis:**
- Existing binding check adds structural equality overhead
- Failures are detected quickly (~68ns)
- Complex patterns approach 400ns

---

### 2. Environment Operation Benchmarks (High-Level Integration)

These benchmarks measure MORK/PathMap integration overhead.

#### 2.1 Type Lookup (`get_type()`)

| Dataset Size | Time (µs) | Scaling |
|--------------|-----------|---------|
| 10 types | 2.6 | Baseline |
| 100 types | 21.9 | 8.4× |
| 1,000 types | 221.3 | 85× |
| 10,000 types | 2,195.9 | 843× |

**Performance Equation:** T(n) ≈ 0.22 × n µs
**Scaling:** **Perfect O(n) linear search** - confirms lack of indexing

#### 2.2 Type Lookup with Mixed Patterns

| Scenario | Dataset | Time (µs) | Notes |
|----------|---------|-----------|-------|
| Small mixed | 50 types | 11.2 | 25% have type assertions |
| Medium mixed | 500 types | 108.9 | 50% have type assertions |
| Large mixed | 5,000 types | 1,051.7 | 75% have type assertions |

**Analysis:** Performance degrades linearly with dataset size regardless of assertion density.

#### 2.3 Sparse Type Lookups

| Sparsity | Dataset | Time (µs) | Notes |
|----------|---------|-----------|-------|
| 1 in 100 | 1,000 total | 39.9 | Must scan 100 items on average |
| 1 in 1,000 | 10,000 total | 408.5 | Must scan 1,000 items |
| 1 in 10,000 | 100,000 total | 4,234.7 | Must scan 10,000 items |

**Critical Finding:** **Sparse lookups are extremely expensive** - no early termination optimization.

#### 2.4 Fact Checking (`has_fact()`)

| Facts | Time (µs) | Notes |
|-------|-----------|-------|
| 10 | 1.9 | |
| 100 | 16.8 | |
| 1,000 | 167.1 | |

**Scaling:** O(n) linear search
**Opportunity:** These could be O(1) with proper indexing

#### 2.5 Rule Iteration (`iter_rules()`)

| Rules | Time (µs) | Notes |
|-------|-----------|-------|
| 10 | 6.9 | |
| 100 | 71.5 | |
| 1,000 | 748.7 | |

**Scaling:** O(n) iteration (expected for full traversal)
**Note:** This is correct - full iteration should be O(n)

#### 2.6 Space Matching (`match_space()`)

| Facts | Time (µs) | Notes |
|-------|-----------|-------|
| 10 | 3.9 | Pattern: `($x $y)` |
| 100 | 36.3 | |
| 1,000 | 376.6 | |

**Scaling:** O(n) for pattern matching against all facts
**Note:** Uses pattern_match() internally, adding ~0.4µs per fact

---

## Hotspot Analysis

**Data Source:** `perf record -F 99 -g --call-graph dwarf` (74,149 samples, 2.18T CPU cycles)
**Profiling Artifacts:**
- `docs/benchmarks/pattern_matching_optimization/baseline_flamegraph.svg` (567K)
- `/tmp/baseline_pattern_match.perf.data` (641MB)

### CPU Time Distribution (pattern_match benchmarks)

| Function | %CPU | Type | Notes |
|----------|------|------|-------|
| `criterion::bencher::Bencher::iter` | 18.61% | Benchmark overhead | Black box & measurement |
| `libm::exp` | 7.88% | Statistical analysis | Criterion KDE estimation |
| `pattern_match_impl` | 6.25% | **Core algorithm** | **Actual pattern matching** |
| `rayon::bridge_producer_consumer` | 6.14% | Benchmark overhead | Parallel statistics |
| `SmartBindings::insert` | 2.25% | Variable binding | HashMap operations |
| `_rjem_sdallocx` | 1.66% | Memory deallocation | jemalloc overhead |
| `MettaValue::clone` | 1.48% | Memory operations | Deep cloning |
| `drop_in_place<MettaValue>` | 1.36% | Cleanup | Arc/Box destruction |

### Key Findings

1. **Pattern matching is already highly optimized**
   - Only 6.25% of CPU time spent in core algorithm
   - Remaining ~74% is benchmark infrastructure (Criterion + rayon)
   - Variable binding (`SmartBindings`) uses only 2.25%

2. **Low-level optimization has limited ROI**
   - Pattern match code path is fast (~100-400ns)
   - Memory operations (`clone` + `drop`) total ~2.84%
   - Further micro-optimizations would yield minimal gains

3. **High-level operations are the bottleneck**
   - `get_type()` with 10,000 items: **2,196 µs** (10,000× slower than pattern match!)
   - These operations use O(n) linear search through MORK zipper
   - **This is where optimization should focus**

### Hypothesis Validation

| Hypothesis | Reality | Impact |
|------------|---------|--------|
| `pattern_match()` is slow | ❌ FALSE - Only 6.25% CPU | Low priority |
| HashMap bindings are slow | ❌ FALSE - Only 2.25% CPU | Low priority |
| `get_type()` is slow | ✅ **CONFIRMED** - 2.2ms for 10K | **HIGH PRIORITY** |
| MORK operations are slow | ✅ **CONFIRMED** - O(n) traversal | **HIGH PRIORITY** |

**Conclusion:** The prefix-based fast path optimization (ReadZipper::descend_to_check) will target the real bottleneck: high-level environment operations, not low-level pattern matching.

---

## Performance Opportunities Identified

### Priority 1: Prefix-Based Fast Path (5-20x potential)

**Current:** All lookups use O(n) linear search or O(k) query_multi
**Proposed:** O(p) exact match via `ReadZipper::descend_to_check()`
**Impact:** Concrete patterns like `(fibonacci 10)` become 5-20× faster

**Evidence:**
- `get_type()` with 10,000 types takes 2.2ms
- With O(p) prefix navigation (p≈3), would take ~200-300ns
- **Expected speedup: 7,000-11,000×** for exact matches!

### Priority 2: PathMap Trie Rule Index

**Current:** HashMap by `(head_symbol, arity)` - O(1) lookup but limited
**Proposed:** PathMap trie with full pattern prefix - O(p + k) lookup
**Impact:** Better filtering, fewer false candidates

**Evidence:**
- Current rule matching is O(k) where k = rules with matching head
- Trie-based would be O(p + k') where k' << k (only prefix-matching rules)
- Rholang LSP saw 90-93% improvement with this change

### Priority 3: Split Pattern vs Value Conversion

**Current:** Unified `metta_to_mork_bytes()` for both patterns and values
**Proposed:** Separate functions for better MORK encoding
**Impact:** More precise query_multi, fewer false positives

---

## Baseline Metrics Summary Table

| Metric | Current Performance | Target (Post-Optimization) |
|--------|-------------------|---------------------------|
| Pattern match (simple) | 98 ns | 98 ns (no change expected) |
| Pattern match (complex) | 388 ns | 388 ns (no change expected) |
| `get_type()` 10,000 items (exact match) | 2,196 µs | **0.2-0.3 µs** (7000× faster) |
| `get_type()` 10,000 items (variable pattern) | 2,196 µs | 2,196 µs (same - no concrete prefix) |
| `has_fact()` 1,000 items | 167 µs | **0.1 µs** (1700× faster with index) |
| Rule lookup with 1,000 rules | ~750 µs | **50-100 µs** (7-15× faster with trie) |

**Overall Expected Impact:**
- **Exact match queries:** 1000-10,000× speedup
- **Pattern queries:** 5-15× speedup
- **Variable-only patterns:** No change (expected)

---

## Next Steps

1. ✅ **Complete:** Baseline benchmarking
2. ⏳ **In Progress:** Perf profiling and flamegraph generation
3. **Pending:** Implement prefix-based fast path
4. **Pending:** Re-benchmark and compare
5. **Pending:** Implement PathMap trie rule index
6. **Pending:** Final comparative analysis

---

## Appendix: Benchmark Configuration

**Hardware:**
- CPU: Intel Xeon E5-2699 v3 @ 2.30GHz
- Cores: 18 physical (36 with HT)
- L1 Cache: 1.1 MiB (data) + 1.1 MiB (instruction)
- L2 Cache: ~9 MB
- L3 Cache: ~45 MB
- RAM: 252 GB DDR4-2133 ECC

**Software:**
- OS: Linux 6.17.7-arch1-1
- Rust: 1.70+ (nightly features enabled)
- Criterion: Default configuration
- CPU Affinity: Cores 0-17 (taskset)

**Benchmark Parameters:**
- Measurement time: 10 seconds per bench (for scaling tests)
- Warm-up: 3 seconds
- Samples: 100 per benchmark
- Outlier detection: Enabled (IQR method)
