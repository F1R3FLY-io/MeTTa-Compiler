# Documentation

This directory contains comprehensive documentation for the MeTTaTron project.

## Directory Structure

```
docs/
├── README.md                # This file
├── ARCHITECTURE.md          # High-level system architecture overview
├── CONTRIBUTING.md          # Contributor guidelines
├── ISSUE_3_SATISFACTION.md  # MVP requirements satisfaction
├── MVP_BACKEND_COMPLETE.md  # MVP implementation status
├── THREADING_MODEL.md       # Threading and parallelization documentation
├── guides/                  # User guides and tutorials
├── reference/               # API and language reference documentation
├── design/                  # Design documents and implementation details
├── testing/                 # Testing documentation
├── optimization/            # Performance optimization documentation
│   ├── experiments/         # Optimization experiment results
│   ├── sessions/            # Optimization session notes and planning
│   └── benchmarks/          # Benchmark results and analysis
├── examples/                # Example documentation
└── archive/                 # Historical documents and archived proposals
```

## User Guides

Documentation for using MeTTaTron:

- **[REPL Guide](guides/REPL_GUIDE.md)** - Comprehensive interactive REPL guide
- **[Reduction Prevention Guide](guides/REDUCTION_PREVENTION.md)** - Using `quote`, `eval`, `catch`, and error handling
- **[Configuration Guide](guides/CONFIGURATION.md)** - Threading and evaluation configuration
- **[Rholang Parser Named Comments](guides/RHOLANG_PARSER_NAMED_COMMENTS.md)** - Parser configuration and named comments feature
- **[Rholang Build Automation](guides/RHOLANG_BUILD_AUTOMATION.md)** - Build automation strategies

## API Reference

Reference documentation for developers:

- **[Backend API Reference](reference/BACKEND_API_REFERENCE.md)** - Complete Rust API documentation
- **[MeTTa Type System Reference](reference/METTA_TYPE_SYSTEM_REFERENCE.md)** - Official type system specification
- **[Built-in Functions Reference](reference/BUILTIN_FUNCTIONS_REFERENCE.md)** - Comprehensive catalog of built-in functions

## Design Documents

Technical design and implementation details:

- **[Backend Implementation](design/BACKEND_IMPLEMENTATION.md)** - Evaluation engine design
- **[REPL Architecture](design/REPL_ARCHITECTURE.md)** - REPL design and implementation
- **[Type System Implementation](design/TYPE_SYSTEM_IMPLEMENTATION.md)** - Type system architecture
- **[Type System Rholang Integration](design/TYPE_SYSTEM_RHOLANG_INTEGRATION.md)** - Type integration patterns
- **[Threading and PathMap Integration](design/THREADING_AND_PATHMAP_INTEGRATION.md)** - Threading model analysis
- **[Threading Improvements](design/THREADING_IMPROVEMENTS.md)** - Implementation guide for threading optimizations
- **[Collection-Aware Parser](design/COLLECTION_AWARE_PARSER.md)** - Collection-aware parsing strategies
- **[Composability Tests](design/COMPOSABILITY_TESTS.md)** - Test composition strategies
- **[MORK PathMap Query Design](design/MORK_PATHMAP_QUERY_DESIGN.md)** - PathMap query optimization
- **[MORK PathMap Operations Mapping](design/MORK_PATHMAP_OPERATIONS_MAPPING.md)** - PathMap operation mapping
- **[PathMap State Design](design/PATHMAP_STATE_DESIGN.md)** - PathMap state management
- **[PathMap Par Integration](design/PATHMAP_PAR_INTEGRATION.md)** - PathMap Par integration
- **[Pattern Matching Optimization](design/PATTERN_MATCHING_OPTIMIZATION.md)** - Pattern matching optimization strategies
- **[Rule Index Optimization](design/RULE_INDEX_OPTIMIZATION.md)** - Rule matching optimization
- **[Optimization Status](design/OPTIMIZATION_STATUS.md)** - Overall optimization status
- **[S-Expression Facts Design](design/SEXPR_FACTS_DESIGN.md)** - Fact storage design
- **[TODO Analysis](design/TODO_ANALYSIS.md)** - Future work and planning

## MORK Documentation

Comprehensive documentation for MORK (the hypergraph processing kernel):

### Core MORK Features ✅
- **[MORK Features Support](mork/MORK_FEATURES_SUPPORT.md)** - Complete feature reference and test results
  - All 9 implemented features (fixed-point, binding threading, priorities, etc.)
  - 17/17 tests passing (ancestor.mm2 verified)
  - Performance characteristics and usage examples
- **[Benchmark Results](mork/BENCHMARK_RESULTS.md)** - Comprehensive performance benchmarks
  - Sub-millisecond performance for typical workloads
  - Scaling analysis and bottleneck identification
  - Hardware utilization and optimization recommendations
- **[Future Enhancements](mork/FUTURE_ENHANCEMENTS.md)** - Planned optimizations and features
  - Performance optimizations (fact indexing, incremental evaluation)
  - Language features (negation, constraints, aggregation)
  - Developer tools (trace/debug mode, visualization)

### Conjunction Pattern
- **[Conjunction Pattern](mork/conjunction-pattern/)** - Deep dive on MORK's comma/conjunction pattern
  - Why uniform conjunctions for unary expressions
  - Parser and evaluator implementation
  - Coalgebra and meta-programming patterns
  - Benefits analysis and comparison with alternatives
  - **[Implementation Details](mork/conjunction-pattern/IMPLEMENTATION.md)** - Technical deep dive
  - **[Completion Summary](mork/conjunction-pattern/COMPLETION_SUMMARY.md)** - Implementation status

### Additional MORK Documentation
- **[Pattern Matching](mork/pattern-matching.md)** - Pattern matching implementation guide
- **[Encoding Strategy](mork/encoding-strategy.md)** - Byte-level encoding specification
- **[Evaluation Engine](mork/evaluation-engine.md)** - Evaluation semantics
- **[Algebraic Operations](mork/algebraic-operations.md)** - Algebraic operations reference
- **[Space Operations](mork/space-operations.md)** - Space manipulation operations
- **[Serialization Guide](mork/serialization-guide.md)** - Serialization formats and examples
- **[Concurrency Guide](mork/concurrency-guide.md)** - Concurrency and parallelization
- **[API Reference](mork/api-reference.md)** - Complete MORK API reference
- **[Performance Guide](mork/performance-guide.md)** - Performance optimization guide
- **[Use Cases](mork/use-cases.md)** - Real-world MORK applications
- **[Implementation Guide](mork/implementation-guide.md)** - Building with MORK
- **[Rholang Integration](mork/rholang-integration.md)** - MORK-Rholang integration

## Testing Documentation

Documentation for testing strategies and frameworks:

- **[Integration Testing Implementation](testing/INTEGRATION_TESTING_IMPLEMENTATION.md)** - Integration test framework and implementation

## Optimization Documentation

Performance optimization work with empirical results:

- **[Performance Summary](optimization/PERFORMANCE_OPTIMIZATION_SUMMARY.md)** - Consolidated optimization results
- **[Scientific Ledger](optimization/SCIENTIFIC_LEDGER.md)** - Scientific method tracking
- **[Optimization README](optimization/README.md)** - Quick reference for all optimizations
- **[Experiments](optimization/experiments/)** - Individual optimization experiment results
- **[Sessions](optimization/sessions/)** - Optimization session notes and planning
- **[Benchmarks](optimization/benchmarks/)** - Benchmark results and analysis

## Status Reports

Project status and milestone documentation:

- **[Architecture Overview](ARCHITECTURE.md)** - High-level system architecture
- **[Issue #3 Satisfaction](ISSUE_3_SATISFACTION.md)** - GitHub Issue #3 MVP requirements analysis
- **[MVP Backend Complete](MVP_BACKEND_COMPLETE.md)** - MVP implementation status and test results
- **[Threading Model](THREADING_MODEL.md)** - Threading and parallelization documentation

## Historical Documentation

For historical status reports and investigation documents, see:

- **`archive/milestones/`** - Historical status reports and completion documents
- **`archive/repl-investigation/`** - REPL implementation research and decisions
- **`archive/proposals/`** - Archived proposals and design explorations
- **`archive/`** - Older summary documents

## Related Documentation

### Integration

For Rholang integration documentation, see:
- **`../integration/README.md`** - Complete Rholang integration guide
- **`../integration/DIRECT_RUST_INTEGRATION.md`** - Direct Rust linking guide (recommended)

### Examples

For code examples, see:
- **`../examples/`** - MeTTa language examples (*.metta)
- **`../examples/`** - Rust backend examples (*.rs)
- **`../examples/`** - Rholang integration examples (*.rho)

### Main Documentation

- **`../README.md`** - Main project README
- **`../.claude/CLAUDE.md`** - Claude Code development guide

## Quick Start

1. **New to MeTTaTron?** Start with the [main README](../README.md)
2. **Want to use the REPL?** See [REPL Guide](guides/REPL_GUIDE.md)
3. **Building a Rust application?** See [Backend API Reference](reference/BACKEND_API_REFERENCE.md)
4. **Integrating with Rholang?** See [Integration Guide](../integration/README.md)
5. **Understanding the type system?** See [Type System Reference](reference/METTA_TYPE_SYSTEM_REFERENCE.md)
6. **Working with MORK?** See [MORK Documentation](#mork-documentation) and [Conjunction Pattern](mork/conjunction-pattern/)

## Contributing

See **[CONTRIBUTING.md](CONTRIBUTING.md)** for full contributor guidelines.

When adding new documentation:

1. **User-facing guides** → `guides/`
2. **API/language references** → `reference/`
3. **Technical design docs** → `design/`
4. **Testing documentation** → `testing/`
5. **Optimization work** → `optimization/`
   - Experiments → `optimization/experiments/`
   - Session notes → `optimization/sessions/`
   - Benchmarks → `optimization/benchmarks/`
6. **Status reports** → Root of `docs/`
7. **Integration docs** → `../integration/`
8. **Historical docs** → `archive/`

---

For the latest updates, see the main repository: https://github.com/F1R3FLY-io/MeTTa-Compiler
