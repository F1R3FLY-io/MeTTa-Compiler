# Changelog

All notable changes to MeTTaTron will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.2.0] - 2025-11-27

### Added
- Tree-Sitter parser replacing hand-written parser with robust grammar
- New `tree-sitter-metta/` grammar with Rust bindings and test corpus
- Enhanced REPL with syntax highlighting, smart indentation, and pattern history
- Fuzzy matching for "did you mean?" suggestions on unknown symbols
- Comprehensive benchmark infrastructure (Criterion, Divan frameworks)
- Logical operators (`and`, `or`, `not`) and improved operator handling
- Extensive MeTTa language documentation:
  - Atom space operations guide
  - Pattern matching reference
  - Type system documentation
  - Order of operations semantics
- MORK integration documentation
- PathMap persistence and threading guides
- Copy-on-Write environment design documentation
- Examples and development scripts

### Changed
- Modular evaluation engine split into specialized modules (`src/backend/eval/`)
- Performance optimizations: SmartBindings (5.9% pattern matching speedup)
- Recursive evaluation converted to iterative trampoline (prevents stack overflow)
- Improved error handling with semantic messages and usage hints
- Refactored MettaValue and MettaState models
- REPL buffer optimized with Rope data structure
- MORK direct conversion cleanup (code simplification)
- S-expression storage aligned with official MeTTa ADD mode semantics

### Fixed
- Stack overflow in deeply nested evaluations (iterative trampoline)
- Overflow check in cartesian product allocation
- Broken `has_fact()` implementation (O(n) → O(1) lookup)
- Various clippy warnings and formatting issues

### Infrastructure
- Integration tests for Rholang bridge
- Extended test coverage for models and integration
- Comprehensive MeTTa benchmark suite with 7 real-world programs

---

## [0.1.2] - 2025-10-21

### Infrastructure
- Added package sanity checks to release workflow
- Fixed nightly workflow alignment with integration/release workflows
- Added artifact download links to nightly build summary
- Fixed package jobs for extracting library files from tarballs
- Fixed RPM and macOS sanity check issues

---

## [0.1.1] - 2025-10-21

### Added
- Arch Linux .pkg.tar.zst package builds in CI/CD
- rholang-cli binary included in all package formats

### Infrastructure
- Implemented comprehensive multi-platform nightly builds and testing
- Fixed Arch packaging and GitHub Release issues
- Fixed CARCH environment variable for Arch package filenames

---

## [0.1.0] - 2025-10-21

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

**Note**: This changelog follows semantic versioning starting from 0.1.0 (October 21, 2025). Earlier development history is available in git commit history.
