# Contributing to MeTTaTron

Thank you for your interest in contributing to MeTTaTron! This document provides guidelines and best practices for contributing to the project.

---

## Table of Contents

1. [Getting Started](#getting-started)
2. [Development Workflow](#development-workflow)
3. [Code Style and Standards](#code-style-and-standards)
4. [Testing Requirements](#testing-requirements)
5. [Performance Considerations](#performance-considerations)
6. [Documentation](#documentation)
7. [Pull Request Process](#pull-request-process)
8. [Optimization Guidelines](#optimization-guidelines)

---

## Getting Started

### Prerequisites

- **Rust toolchain 1.70+** (install via [rustup](https://rustup.rs/))
- **Cargo** (comes with Rust)
- **Git** for version control

No external parser generators or C toolchain required.

### Initial Setup

```bash
# Clone the repository
git clone https://github.com/f1r3fly/MeTTa-Compiler.git
cd MeTTa-Compiler

# Build the project
cargo build --release

# Run tests to verify setup
cargo test

# Run linter
cargo clippy

# Format code
cargo fmt
```

### Project Structure

See `docs/ARCHITECTURE.md` for detailed architecture overview.

Key directories:
- `src/` - Rust source code
  - `src/backend/` - Evaluation engine
  - `src/backend/eval/` - Modular evaluation logic
- `docs/` - Documentation
- `examples/` - MeTTa and Rust examples
- `tests/` - Integration tests
- `benches/` - Performance benchmarks

---

## Development Workflow

### Branch Strategy

- **`main`** - Production-ready code, always passes all tests
- **Feature branches** - `feature/your-feature-name`
- **Bug fixes** - `fix/issue-description`
- **Optimization work** - `perf/optimization-name`

### Creating a Feature Branch

```bash
# Create and switch to new branch
git checkout -b feature/my-new-feature

# Make your changes
# ... edit files ...

# Run tests
cargo test

# Run linter
cargo clippy

# Format code
cargo fmt

# Commit with clear message
git add .
git commit -m "feat: Add new feature X"

# Push to remote
git push origin feature/my-new-feature
```

### Commit Message Format

Follow conventional commits format:

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

**Types**:
- `feat:` - New feature
- `fix:` - Bug fix
- `perf:` - Performance improvement
- `refactor:` - Code refactoring
- `docs:` - Documentation changes
- `test:` - Test additions or changes
- `chore:` - Maintenance tasks

**Examples**:
```
feat(eval): Add new special form 'let'
fix(parser): Handle escaped quotes in strings
perf(environment): Optimize rule lookup with HashMap index
docs(architecture): Add high-level system diagram
test(integration): Add robot planning test cases
```

---

## Code Style and Standards

### Rust Style Guidelines

**Follow standard Rust conventions**:
- Use `cargo fmt` for automatic formatting (enforced)
- Fix all `cargo clippy` warnings (required for CI)
- Prefer explicit types in public APIs
- Document public APIs with doc comments (`///`)
- Use descriptive variable names

**Example**:
```rust
/// Evaluates a MeTTa expression with pattern matching and rule application.
///
/// # Arguments
/// * `expr` - The expression to evaluate
/// * `depth` - Current recursion depth (for cycle detection)
///
/// # Returns
/// * `Ok(Vec<MettaValue>)` - Evaluation results (may be multiple due to pattern matching)
/// * `Err(String)` - Error message if evaluation fails
///
/// # Example
/// ```rust
/// let expr = MettaValue::Symbol("fib".to_string());
/// let results = env.eval(&expr, 0)?;
/// ```
pub fn eval(&self, expr: &MettaValue, depth: usize) -> Result<Vec<MettaValue>, String> {
    // Implementation...
}
```

### Naming Conventions

- **Functions**: `snake_case`
- **Types/Structs**: `PascalCase`
- **Constants**: `SCREAMING_SNAKE_CASE`
- **Modules**: `snake_case`

### Error Handling

- Use `Result<T, String>` for operations that can fail
- Provide descriptive error messages
- Propagate errors with `?` operator
- Use `anyhow` or `thiserror` for complex error types (if needed)

---

## Testing Requirements

### Test Coverage

**All code changes must include tests**:
- Unit tests for new functions/methods
- Integration tests for new features
- Regression tests for bug fixes

### Running Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_function_name

# Run tests in specific module
cargo test backend::eval::tests
```

### Writing Tests

**Unit Test Example**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_simple_expression() {
        let mut env = Environment::new();
        env.add_rule(&parse("(= (fib 0) 1)").unwrap()).unwrap();

        let expr = parse("(fib 0)").unwrap();
        let results = env.eval(&expr, 0).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));
    }
}
```

**Integration Test Example** (in `tests/`):
```rust
use mettatron::{Environment, MettaValue};

#[test]
fn test_fibonacci_evaluation() {
    let mut env = Environment::new();

    // Load rules from file
    env.load_file("examples/fibonacci.metta").unwrap();

    // Evaluate expression
    let expr = parse("(fib 5)").unwrap();
    let results = env.eval(&expr, 0).unwrap();

    assert_eq!(results[0], MettaValue::Long(5));
}
```

### Test Quality Standards

- **Clear test names** describing what is tested
- **Arrange-Act-Assert** pattern
- **Test edge cases**: empty inputs, large inputs, error conditions
- **No flaky tests**: Tests must be deterministic

---

## Performance Considerations

### When to Optimize

**Follow these principles**:
1. **Profile first, optimize second** - Use benchmarks and profiling tools
2. **Measure impact** - Quantify speedup with empirical data
3. **Document trade-offs** - Memory vs speed, complexity vs performance
4. **Follow scientific method** - See `docs/optimization/SCIENTIFIC_LEDGER.md`

### Benchmarking

**Add benchmarks for performance-critical code**:

```rust
// benches/my_benchmark.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mettatron::Environment;

fn benchmark_rule_lookup(c: &mut Criterion) {
    let mut env = Environment::new();
    // ... setup ...

    c.bench_function("rule_lookup_1000", |b| {
        b.iter(|| {
            env.lookup_rules(black_box("fib"), black_box(1))
        });
    });
}

criterion_group!(benches, benchmark_rule_lookup);
criterion_main!(benches);
```

**Run benchmarks**:
```bash
# Run specific benchmark
cargo bench --bench my_benchmark

# Run all benchmarks
cargo bench

# With CPU affinity (for consistent results)
taskset -c 0-17 cargo bench
```

### Performance Documentation

**Document all optimizations** in `docs/optimization/`:
- **Hypothesis**: What you expect to improve and why
- **Implementation**: Changes made
- **Measurements**: Benchmark results with statistical significance
- **Analysis**: Why the speedup occurred (or didn't)
- **Trade-offs**: Memory, complexity, maintainability costs

See examples:
- `docs/optimization/experiments/VARIANT_C_RESULTS_2025-11-11.md`
- `docs/optimization/PERFORMANCE_OPTIMIZATION_SUMMARY.md`

---

## Documentation

### Documentation Standards

**All public APIs must be documented**:
- Use doc comments (`///`) for public functions, structs, enums
- Include usage examples in doc comments
- Update relevant documentation in `docs/` for architectural changes

### Documentation Categories

1. **API Documentation** (in code via doc comments)
   - Public functions, types, modules
   - Usage examples
   - Safety notes (for `unsafe` code)

2. **User Guides** (`docs/guides/`)
   - How-to guides for common tasks
   - Configuration documentation
   - REPL usage

3. **Design Documents** (`docs/design/`)
   - Architecture decisions
   - Design rationale
   - Implementation details

4. **Reference Documentation** (`docs/reference/`)
   - MeTTa language reference
   - Built-in functions catalog
   - Type system specification

### Updating Documentation

**When to update docs**:
- Adding new features → Update user guides and API docs
- Changing architecture → Update `docs/ARCHITECTURE.md`
- Performance improvements → Document in `docs/optimization/`
- Bug fixes → Update relevant troubleshooting sections

---

## Pull Request Process

### Before Submitting

**Checklist**:
- [ ] All tests pass (`cargo test`)
- [ ] No clippy warnings (`cargo clippy`)
- [ ] Code is formatted (`cargo fmt`)
- [ ] New code has tests
- [ ] Documentation is updated
- [ ] Benchmarks added (if performance-related)
- [ ] Commit messages follow conventional format

### PR Description Template

```markdown
## Description
Brief description of changes

## Type of Change
- [ ] Bug fix (non-breaking change fixing an issue)
- [ ] New feature (non-breaking change adding functionality)
- [ ] Breaking change (fix or feature causing existing functionality to change)
- [ ] Performance improvement
- [ ] Documentation update

## Testing
Describe the tests you added/ran

## Performance Impact
(If applicable) Include benchmark results

## Related Issues
Closes #123
```

### Review Process

1. **Submit PR** with clear description
2. **CI checks** must pass (tests, clippy, fmt)
3. **Code review** by maintainer(s)
4. **Address feedback** with additional commits
5. **Approval** → Merge to main

### Review Criteria

Reviewers will check:
- **Correctness**: Does it solve the problem?
- **Tests**: Adequate test coverage?
- **Style**: Follows Rust conventions?
- **Performance**: No unexpected regressions?
- **Documentation**: Clear and complete?
- **Maintainability**: Easy to understand and modify?

---

## Optimization Guidelines

### Scientific Method for Optimizations

Follow this process for all performance work:

1. **Observation**: Identify bottleneck via profiling
2. **Hypothesis**: Predict what optimization will improve and by how much
3. **Implementation**: Make the change
4. **Measurement**: Benchmark with statistical rigor
5. **Analysis**: Explain why speedup occurred (or didn't)
6. **Documentation**: Record findings for future reference

See: `docs/optimization/SCIENTIFIC_LEDGER.md`

### Profiling Tools

```bash
# CPU profiling with perf
perf record --call-graph=dwarf cargo bench --bench my_benchmark
perf report

# Generate flamegraph
cargo install flamegraph
cargo flamegraph --bench my_benchmark
```

### Benchmark Best Practices

- **CPU affinity**: Use `taskset` for consistent results
- **Statistical significance**: Run multiple iterations (Criterion default: 100)
- **Isolate variables**: Test one change at a time
- **Document hardware**: Record CPU, memory, OS for reproducibility

### Optimization Priorities

**Order of importance**:
1. **Correctness** - Never sacrifice correctness for performance
2. **Algorithmic complexity** - O(n²) → O(n log n) wins over micro-optimizations
3. **Data structures** - Choose appropriate structures (HashMap vs Vec, etc.)
4. **Parallelization** - Leverage multi-core where possible
5. **Micro-optimizations** - Only after profiling shows specific hotspots

---

## Communication

### Asking Questions

- **GitHub Issues** - For bug reports and feature requests
- **GitHub Discussions** - For questions and general discussion
- **Pull Request comments** - For code-specific questions

### Reporting Bugs

**Include**:
- MeTTaTron version / commit hash
- Rust version (`rustc --version`)
- Operating system
- Steps to reproduce
- Expected vs actual behavior
- Relevant logs/error messages

---

## Additional Resources

- **Architecture**: `docs/ARCHITECTURE.md`
- **Threading Model**: `docs/THREADING_MODEL.md`
- **User Guides**: `docs/guides/`
- **API Reference**: `docs/reference/`
- **Optimization**: `docs/optimization/`
- **Examples**: `examples/`

---

## Recognition

Contributors will be recognized in:
- Git commit history (preserved via `Co-Authored-By`)
- Release notes
- `CONTRIBUTORS.md` (if created)

---

**Thank you for contributing to MeTTaTron!**

For questions or clarifications, please open a GitHub issue or discussion.

---

**Status**: ✅ Current
**Last Updated**: 2025-11-12
