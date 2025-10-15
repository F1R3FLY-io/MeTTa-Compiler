# Documentation

This directory contains comprehensive documentation for the MeTTaTron project.

## Directory Structure

```
docs/
├── README.md               # This file
├── guides/                 # User guides and tutorials
├── reference/              # API and language reference documentation
├── design/                 # Design documents and implementation details
├── ISSUE_3_SATISFACTION.md # MVP requirements satisfaction
└── MVP_BACKEND_COMPLETE.md # MVP implementation status
```

## User Guides

Documentation for using MeTTaTron:

- **[REPL Usage Guide](guides/REPL_USAGE.md)** - Complete guide to the interactive REPL
- **[Reduction Prevention Guide](guides/REDUCTION_PREVENTION.md)** - Using `quote`, `eval`, `catch`, and error handling

## API Reference

Reference documentation for developers:

- **[Backend API Reference](reference/BACKEND_API_REFERENCE.md)** - Complete Rust API documentation
- **[MeTTa Type System Reference](reference/METTA_TYPE_SYSTEM_REFERENCE.md)** - Official type system specification
- **[Type System Analysis](reference/TYPE_SYSTEM_ANALYSIS.md)** - Type system implementation analysis

## Design Documents

Technical design and implementation details:

- **[Backend Implementation](design/BACKEND_IMPLEMENTATION.md)** - Evaluation engine design
- **[Type System Implementation](design/TYPE_SYSTEM_IMPLEMENTATION.md)** - Type system architecture
- **[Type System Rholang Integration](design/TYPE_SYSTEM_RHOLANG_INTEGRATION.md)** - Type integration patterns
- **[MORK PathMap Query Design](design/MORK_PATHMAP_QUERY_DESIGN.md)** - PathMap query optimization
- **[Rule Index Optimization](design/RULE_INDEX_OPTIMIZATION.md)** - Rule matching optimization
- **[S-Expression Facts Design](design/SEXPR_FACTS_DESIGN.md)** - Fact storage design
- **[TODO Analysis](design/TODO_ANALYSIS.md)** - Future work and planning

## Status Reports

Project status and milestone documentation:

- **[Issue #3 Satisfaction](ISSUE_3_SATISFACTION.md)** - GitHub Issue #3 MVP requirements analysis
- **[MVP Backend Complete](MVP_BACKEND_COMPLETE.md)** - MVP implementation status and test results

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
- **`../CLAUDE.md`** - Claude Code development guide

## Quick Start

1. **New to MeTTaTron?** Start with the [main README](../README.md)
2. **Want to use the REPL?** See [REPL Usage Guide](guides/REPL_USAGE.md)
3. **Building a Rust application?** See [Backend API Reference](reference/BACKEND_API_REFERENCE.md)
4. **Integrating with Rholang?** See [Integration Guide](../integration/README.md)
5. **Understanding the type system?** See [Type System Reference](reference/METTA_TYPE_SYSTEM_REFERENCE.md)

## Contributing

When adding new documentation:

1. **User-facing guides** → `guides/`
2. **API/language references** → `reference/`
3. **Technical design docs** → `design/`
4. **Status reports** → Root of `docs/`
5. **Integration docs** → `../integration/`

---

For the latest updates, see the main repository: https://github.com/F1R3FLY-io/MeTTa-Compiler
