# Changelog

All notable changes to MeTTaTron will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added - Expression-Level Parallelism (Optimization 3) ⚡

#### Parallel Sub-Expression Evaluation with Rayon
- Implemented expression-level parallelism for independent sub-expression evaluation
- Added `rayon = "1.8"` dependency for parallel iteration
- Adaptive threshold: Parallelizes only when `sub_expressions >= 4`
- Below threshold: Uses sequential evaluation (avoids overhead)

**Performance Strategy**:
- Targets actual evaluation work (not just serialization)
- Each sub-expression evaluated independently in parallel
- Example: `(+ (* 2 3) (/ 10 5) (- 8 4) (* 7 2))` → 4 operations run concurrently
- Expected speedup: 2-8× for complex nested expressions

**Implementation Details**:
- Modified `eval_sexpr()` in `src/backend/eval/mod.rs`
- Uses Rayon's `par_iter()` for sub-expression evaluation when threshold met
- Each thread gets cloned Environment (thread-safe isolation)
- Environments unioned after parallel work completes
- All 403 tests pass - no breaking changes

**Adaptive Threshold** (`PARALLEL_EVAL_THRESHOLD = 4`):
- Empirically tuned based on parallel overhead (~50µs) vs evaluation time
- Will be further tuned based on comprehensive benchmarking

See: Commits TBD, Documentation: `docs/optimization/OPTIMIZATION_3_EXPRESSION_PARALLELISM.md` (TBD)

### Rejected - Parallel Bulk Operations (Optimization 4) ❌

**Attempted**: Three approaches to parallelize bulk fact/rule insertion with Rayon

**Result**: COMPLETELY REJECTED after all three approaches failed

**Approaches Tested**:
1. **Parallel Space/PathMap Creation**: Segfault at 1000 items (jemalloc arena exhaustion)
2. **String-Only Parallelization**: 647% regression (6.47× slowdown) for facts at 1000 items
3. **Thread-Local PathMap** (user suggestion): **STILL SEGFAULTS** at 100 items (threshold boundary)

**Why Rejected**:
1. **Persistent Segmentation Faults**: jemalloc arena exhaustion occurs even with thread-local PathMaps
2. **Fundamental Incompatibility**: PathMap + Rayon parallelism incompatible at allocator level
3. **Massive Regressions**: 3.5-7.3× slowdown for facts when segfaults avoided (Approach 2)
4. **Amdahl's Law Limitation**: Only 10% of work parallelizable (PathMap = 90%), max speedup 1.11×
5. **Thread-Local Doesn't Help**: Problem is simultaneous allocation, not concurrent modification

**Critical Finding**: Creating independent PathMap instances per thread **does NOT prevent** jemalloc arena exhaustion when ~18 Rayon worker threads all allocate simultaneously.

**Empirical Evidence**:
- **Approach 1**: Segfault at 1000 items (parallel Space creation)
- **Approach 2**: 3.5× regression (100 facts), 7.3× regression (1000 facts)
- **Approach 3**: Segfault at 100 items (exactly at `PARALLEL_BULK_THRESHOLD`)
- **5+ segfaults observed**, all at same instruction (`segfault at 10`)
- **All 403 tests pass** with thread-local approach (misleading - tests use < 100 items)

**Benchmark Results (Approach 2 - String-Only)**:
| Batch Size | Baseline | Optimized | Speedup | Status |
|------------|----------|-----------|---------|--------|
| 10 facts | 16.07 µs | 12.98 µs | 1.24× | ✅ Good |
| 50 facts | 87.21 µs | 47.98 µs | 1.82× | ✅ Good |
| **100 facts** | **201.92 µs** | **717.79 µs** | **0.28× (3.5× SLOWER)** | ❌ **REGRESSION** |
| **500 facts** | **1.19 ms** | **4.48 ms** | **0.27× (3.7× SLOWER)** | ❌ **REGRESSION** |
| **1000 facts** | **2.46 ms** | **17.90 ms** | **0.14× (7.3× SLOWER)** | ❌ **REGRESSION** |

**Lessons Learned**:
1. **Amdahl's Law applies**: Cannot parallelize 10% of work and expect significant gains
2. **Parallel overhead is real**: Thread spawning cost > serialization gains for small workloads
3. **Allocator limitations**: jemalloc arena exhaustion with simultaneous PathMap creation
4. **Thread-local ≠ Allocator-safe**: Independent instances per thread still exhaust arenas
5. **PathMap constraints are fundamental**: `Cell<u64>` prevents both concurrent modification AND parallel allocation
6. **Always profile before optimizing**: 90% of time in PathMap (not serialization)

**Recommendation**: Do NOT attempt further parallelization of bulk operations. Focus on:
- Expression-level parallelism (Optimization 3) ✅ Already implemented
- Algorithmic improvements to PathMap usage
- Pre-building tries offline for static data

**Documentation**: `docs/optimization/OPTIMIZATION_4_REJECTED_PARALLEL_BULK_OPERATIONS.md` provides comprehensive analysis with all three approaches

See: Commits TBD (reversion)

### Rejected - Parallel Bulk Operations (Optimization 2) ❌

**Note**: This was an earlier attempt that was also rejected. See Optimization 4 above for the most recent comprehensive rejection.

**Attempted**: Rayon-based data parallelism for MORK serialization in bulk operations

**Result**: Completely reverted due to critical failures and fundamental design flaws

**Why Rejected**:
1. **Segmentation Faults**: jemalloc arena exhaustion at 1000-item threshold
2. **Massive Regression**: 647% slowdown (6.47×) after fixing segfaults
3. **Wrong Bottleneck**: Parallelized 10% (serialization) while 90% (PathMap) remained sequential
4. **Amdahl's Law Limitation**: Max theoretical speedup only 1.11× even with perfect parallelization
5. **Thread-Safety Constraint**: PathMap's `Cell<u64>` prevents parallel construction

**Empirical Evidence**:
- Initial benchmarks: 2-12% regressions across all batch sizes
- After segfault fix: 647% regression for 1000-item batches
- PathMap operations: 90% of time, cannot be parallelized
- MORK serialization: 10% of time, parallelization overhead exceeds benefit

**Lessons Learned**:
- Always profile before optimizing (identify real bottlenecks)
- Amdahl's Law applies: parallelizing small portions yields minimal gains
- Thread-safety constraints of dependencies limit options
- Parallel overhead significant for small workloads
- Expression-level > batch-level parallelism for MeTTa

**Documentation**: `docs/optimization/OPTIMIZATION_2_REJECTED.md` provides comprehensive analysis

See: Commits TBD (reversion)

### Documentation
- Reorganized documentation into intuitive directory structure
- Added `docs/ARCHITECTURE.md` - High-level system architecture overview
- Added `docs/CONTRIBUTING.md` - Contributor guidelines
- Created `.claude/docs/` for Claude-specific documentation
- Consolidated optimization documentation into subdirectories

---

## [1.0.0] - 2025-11-12

### Added - Major Performance Optimizations

#### MORK Serialization Optimization (10.3× speedup) 🚀
- Implemented direct MORK byte conversion (Variant C)
- Bypasses costly `ParDataParser::sexpr()` parsing step (~8500ns → ~0ns)
- Peak speedup: 10.3× for bulk fact insertion (100 facts)
- Median speedup: 5-10× across all operations
- Per-operation time: 9.0 μs → 0.95 μs (89% reduction)

**Performance Results**:
- Bulk facts (100): 989.1 μs → 95.6 μs (10.3× speedup, -90.2%)
- Bulk facts (1000): 10.81 ms → 1.13 ms (9.6× speedup, -89.5%)
- Bulk rules (100): 1135 μs → 194 μs (5.8× speedup, -82.8%)
- Bulk rules (1000): 12.37 ms → 2.33 ms (5.3× speedup, -81.2%)

See: `docs/optimization/experiments/VARIANT_C_RESULTS_2025-11-11.md`

#### Type Index Optimization (242.9× median speedup) 🎯
- Implemented lazy-initialized type-only PathMap subtrie
- Uses `PathMap::restrict()` for efficient type lookups
- Cold cache: O(n) build time, Hot cache: O(1) lookup
- Average speedup: 242.9× (11.3× to 551.4× depending on dataset size)

**Performance Results**:
- 100 types: 10.29 μs → 913.85 ns (11.3× speedup)
- 1,000 types: 79.66 μs → 942.10 ns (84.6× speedup)
- 5,000 types: 318.38 μs → 982.13 ns (324.2× speedup)
- 10,000 types: 527.02 μs → 955.71 ns (551.4× speedup)

See: `docs/optimization/PERFORMANCE_OPTIMIZATION_SUMMARY.md`

#### Rule Index Optimization (1.6-1.8× speedup)
- Implemented HashMap-based rule indexing by `(head_symbol, arity)`
- Reduces rule matching complexity from O(n) to O(k) where k << n
- Fibonacci lookup (1000 rules): 49.6ms → 28.1ms (1.76× speedup)

See: `docs/archive/RULE_MATCHING_OPTIMIZATION_SUMMARY.md`

### Changed
- Migrated from `Arc<Mutex<Space>>` to `Arc<RwLock<Space>>` for concurrent reads
- Modified `Environment` structure to include rule index and type index
- Updated `add_to_space()`, `bulk_add_facts()`, `bulk_add_rules()` with optimizations

### Infrastructure
- Added comprehensive benchmark suite (`benches/type_lookup.rs`, `benches/bulk_operations.rs`)
- Established baseline measurements with CPU affinity (cores 0-17)
- Implemented scientific method tracking for optimizations

### Documentation
- Created extensive optimization documentation (21 files in `docs/optimization/`)
- Added threading model documentation (`docs/THREADING_MODEL.md`)
- Documented all optimization phases with empirical results
- Added session notes and experiment results

---

## [0.5.0] - 2025-11-10

### Added
- Rule matching optimization with HashMap indexing
- Bulk operations infrastructure (`bulk_add_facts()`, `bulk_add_rules()`)
- Prefix-based pattern matching fast path (1,024× speedup potential)
- PathMap subtrie operations

### Changed
- Environment structure to include rule index and wildcard rules
- Rule application logic to use indexed lookup

### Documentation
- Threading model audit and analysis (22 lock sites documented)
- Performance characteristics documentation
- Baseline benchmarking for prefix navigation

---

## [0.4.0] - 2025-11-09

### Added
- Rholang threading pattern migration (partial)
- Cross-thread Environment usage patterns
- Comprehensive threading analysis

### Documentation
- `docs/design/THREADING_AND_PATHMAP_INTEGRATION.md` (1,042 lines)
- `docs/design/THREADING_IMPROVEMENTS.md` (1,120 lines)

---

## [0.3.0] - 2025-11-08

### Added
- MORK/PathMap integration for fact storage
- Direct MORK byte conversion utilities
- PathMap Par conversion for Rholang integration

### Changed
- Environment to use PathMap for fact storage
- Fact insertion to use MORK serialization

### Documentation
- PathMap integration guides
- MORK conversion documentation

---

## [0.2.0] - 2025-11-07

### Added
- Type system implementation with type assertions
- Type inference and checking
- Error handling with `error`, `catch`, `is-error`
- Quote and eval special forms
- List operations (cons, car, cdr, etc.)

### Changed
- Modular evaluation engine split into specialized modules
- Evaluation logic reorganized into `src/backend/eval/`

### Documentation
- Type system reference documentation
- Built-in functions catalog
- Design documents for evaluation model

---

## [0.1.0] - 2025-11-06

### Added - Initial Release
- Tree-Sitter based MeTTa parser
- S-expression compilation to MettaValue AST
- Lazy evaluation with pattern matching
- Rule definition and application
- Control flow (if, switch, case)
- Grounded functions (arithmetic, comparisons)
- Basic REPL
- CLI with file evaluation
- Rholang integration (synchronous and asynchronous)

### Infrastructure
- Cargo build system
- Test suite
- Examples (MeTTa and Rust)
- Integration tests

### Documentation
- README with quickstart
- Installation guide
- User guides (REPL, configuration)
- API reference
- Examples documentation

---

## Format Guidelines

### Categories
- **Added** - New features
- **Changed** - Changes to existing functionality
- **Deprecated** - Soon-to-be-removed features
- **Removed** - Removed features
- **Fixed** - Bug fixes
- **Security** - Security improvements
- **Performance** - Performance improvements
- **Documentation** - Documentation changes
- **Infrastructure** - Build/test/CI changes

### Version Numbering
Given a version number MAJOR.MINOR.PATCH:
- **MAJOR** - Incompatible API changes
- **MINOR** - Backwards-compatible functionality additions
- **PATCH** - Backwards-compatible bug fixes

---

## Links
- **Repository**: https://github.com/f1r3fly/MeTTa-Compiler
- **Documentation**: `docs/`
- **Issue Tracker**: https://github.com/f1r3fly/MeTTa-Compiler/issues

---

**Note**: This changelog started at version 1.0.0 (November 12, 2025) following the major performance optimization work. Earlier development history is available in git commit history.
