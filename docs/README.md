# Documentation

This directory contains comprehensive documentation for the MeTTaTron project.

## Directory Structure

```
docs/
├── README.md               # This file
├── ISSUE_3_SATISFACTION.md # MVP requirements satisfaction
├── MVP_BACKEND_COMPLETE.md # MVP implementation status
├── THREADING_MODEL.md      # Threading and parallelization documentation
├── guides/                 # User guides and tutorials
├── reference/              # API and language reference documentation
├── design/                 # Design documents and implementation details
├── testing/                # Testing documentation
└── archive/                # Historical documents and status reports
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

## Testing Documentation

Documentation for testing strategies and frameworks:

- **[Integration Testing Implementation](testing/INTEGRATION_TESTING_IMPLEMENTATION.md)** - Integration test framework and implementation

## Status Reports

Project status and milestone documentation:

- **[Issue #3 Satisfaction](ISSUE_3_SATISFACTION.md)** - GitHub Issue #3 MVP requirements analysis
- **[MVP Backend Complete](MVP_BACKEND_COMPLETE.md)** - MVP implementation status and test results
- **[Threading Model](THREADING_MODEL.md)** - Threading and parallelization documentation

## Historical Documentation

For historical status reports and investigation documents, see:

- **`archive/milestones/`** - Historical status reports and completion documents
- **`archive/repl-investigation/`** - REPL implementation research and decisions

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

## Contributing

When adding new documentation:

1. **User-facing guides** → `guides/`
2. **API/language references** → `reference/`
3. **Technical design docs** → `design/`
4. **Testing documentation** → `testing/`
5. **Status reports** → Root of `docs/`
6. **Integration docs** → `../integration/`
7. **Historical docs** → `archive/`

---

For the latest updates, see the main repository: https://github.com/F1R3FLY-io/MeTTa-Compiler
