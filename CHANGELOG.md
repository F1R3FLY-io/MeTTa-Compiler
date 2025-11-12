# Changelog

All notable changes to MeTTaTron will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

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
