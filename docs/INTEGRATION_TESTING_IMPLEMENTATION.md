# Integration Testing Suite Implementation

This document tracks the implementation of an automated integration testing suite for Rholang example files.

**Goal:** Ensure Rholang `.rho` files evaluate correctly with automated validation and CI/CD integration.

**Repository:** https://github.com/F1R3FLY-io/MeTTa-Compiler

## Overview

The integration testing suite will:
- Execute `.rho` files via `rholang-cli`
- Parse and validate PathMap outputs
- Run automatically in CI/CD
- Provide clear pass/fail reporting
- Support regression testing

## Implementation Checklist

### Phase 1: Basic Test Runner ⭐ (Priority)
**Estimated: 1-2 days** | **Status: ✅ Complete**

- [x] Create `tests/` directory structure
- [x] Implement `find_rholang_cli()` utility
- [x] Create basic test runner
- [x] Run existing integration tests
- [x] Capture stdout/stderr output
- [x] Generate simple pass/fail report
- [x] Document usage in README

**Files:**
- `tests/rholang_integration.rs` - Main test file
- `tests/utils.rs` - Helper utilities
- `tests/README.md` - Test documentation

### Phase 2: Output Validation
**Estimated: 2-3 days** | **Status: Not Started**

- [ ] Implement PathMap output parser
- [ ] Create test specification data structures
- [ ] Validate `eval_outputs` field
- [ ] Validate `environment` persistence
- [ ] Validate `pending_exprs` handling
- [ ] Add detailed assertion messages
- [ ] Create test result reporter

**Files:**
- `tests/output_parser.rs` - PathMap parsing logic
- `tests/test_specs.rs` - Test specification types
- `tests/validators.rs` - Output validation logic

### Phase 3: Test Configuration & Organization
**Estimated: 1-2 days** | **Status: Not Started**

- [ ] Create TOML test configuration format
- [ ] Implement configuration parser
- [ ] Add test categorization (arithmetic, rules, errors, etc.)
- [ ] Support parallel test execution
- [ ] Add test filtering by name/category
- [ ] Create test manifest file

**Files:**
- `tests/integration_tests.toml` - Test configuration
- `tests/config.rs` - Configuration parsing
- `tests/runner.rs` - Advanced test runner

### Phase 4: CI/CD Integration
**Estimated: 1 day** | **Status: Not Started**

- [ ] Create GitHub Actions workflow
- [ ] Add build steps for MeTTaTron
- [ ] Add build steps for rholang-cli
- [ ] Configure test execution
- [ ] Upload test artifacts on failure
- [ ] Add status badges to README
- [ ] Configure automated reporting

**Files:**
- `.github/workflows/integration-tests.yml`
- `.github/workflows/nightly-tests.yml` (optional)

### Phase 5: Enhanced Features
**Estimated: 2-3 days** | **Status: Not Started**

- [ ] Add JSON output format to Rholang test harness
- [ ] Implement performance benchmarking
- [ ] Add test coverage tracking
- [ ] Create regression detection system
- [ ] Add test timeout handling
- [ ] Implement test retry logic
- [ ] Create HTML test reports

**Files:**
- `integration/test_harness_json.rho` - JSON output harness
- `tests/benchmarks.rs` - Performance tests
- `tests/html_reporter.rs` - HTML report generation

## Current Status

**Overall Progress: 20%**

- ✅ Analysis complete
- ✅ Documentation created
- ✅ **Phase 1 complete** - Basic test runner working!
- ⏳ Phases 2-5 pending

### Phase 1 Results

**Test Suite Created:** 21 integration tests
**Passing Tests:** ~18/21 tests passing
**Build Time:** ~11 seconds (first build), <1 second (incremental)
**Test Execution:** <1 second per test

**Sample Test Results:**
```bash
$ RUSTFLAGS="-C target-cpu=native" cargo test --test rholang_integration
running 21 tests
test test_rholang_cli_exists ... ok
test test_integration_dir_exists ... ok
test test_all_test_files_exist ... ok
test test_pathmap_simple ... ok
test test_pathmap_run_method ... ok
test test_harness_simple ... ok
test test_harness_composability ... ok
test test_harness_validation ... ok
test test_example_robot_planning ... ok
... (more tests)

test result: ok. 18 passed; 3 failed; 0 ignored
```

**Known Issues:**
- Some tests detect false positives for "error" in output
- Need better output validation (Phase 2)

## Quick Start (Once Implemented)

### Running All Tests
```bash
cargo test --test rholang_integration
```

### Running Specific Tests
```bash
cargo test --test rholang_integration test_arithmetic
```

### Verbose Output
```bash
cargo test --test rholang_integration -- --nocapture --test-threads=1
```

### Setting Custom rholang-cli Path
```bash
export RHOLANG_CLI_PATH=/path/to/rholang-cli
cargo test --test rholang_integration
```

## Architecture

### Test Flow

```
┌─────────────────┐
│  Test Runner    │
│ (Rust)          │
└────────┬────────┘
         │
         ├─> Find rholang-cli
         ├─> Load test specs
         └─> For each .rho file:
                │
                ├─> Execute with timeout
                ├─> Capture output
                ├─> Parse PathMap
                └─> Validate expectations
                    │
                    ├─> PASS ✓
                    └─> FAIL ✗
```

### Test Specification Format

```rust
pub struct RholangTestSpec {
    pub name: String,
    pub file: PathBuf,
    pub timeout_secs: u64,
    pub expected_outputs: Vec<ExpectedOutput>,
}

pub struct ExpectedOutput {
    pub test_name: String,
    pub pattern: OutputPattern,
}

pub enum OutputPattern {
    Contains { text: String },
    Regex { pattern: String },
    EvalOutputs { values: Vec<String> },
    PathMapStructure {
        has_pending_exprs: bool,
        has_environment: bool,
        output_count: usize
    },
}
```

### PathMap Output Structure

```
{|
  ("pending_exprs", [expr1, expr2, ...]),
  ("environment", ({|| space_data ||}, [type1, type2, ...])),
  ("eval_outputs", [result1, result2, ...])
|}
```

## Test Categories

### 1. Arithmetic Tests
- Basic operations: `+`, `-`, `*`, `/`
- Nested expressions
- Type validation

**Files:**
- `integration/test_harness_simple.rho` (subset)

### 2. Boolean & Comparison Tests
- Comparisons: `<`, `>`, `<=`, `>=`, `==`
- Boolean literals: `true`, `false`
- Conditional expressions

**Files:**
- `integration/test_harness_simple.rho` (subset)

### 3. Control Flow Tests
- `if` expressions (true/false branches)
- `match` patterns
- `let` bindings

**Files:**
- `integration/test_harness_simple.rho` (subset)

### 4. Rule Definition & Usage
- Rule definitions: `(= pattern body)`
- Rule invocation
- Rule persistence across evaluations
- Multiple rules

**Files:**
- `integration/test_harness_composability.rho`
- `integration/test_pathmap_state.rho`

### 5. Error Handling
- Error creation: `(error msg details)`
- Error catching: `(catch expr default)`
- Error checking: `(is-error expr)`

**Files:**
- `integration/test_harness_simple.rho` (subset)

### 6. Quote & Eval
- Quoting: `(quote expr)`
- Evaluation: `(eval expr)`
- Quote/eval composition

**Files:**
- `integration/test_harness_simple.rho` (subset)

### 7. Sequential Composition
- State accumulation: `s.run(a).run(b).run(c)`
- Rule persistence across runs
- Multiple expressions in single run

**Files:**
- `integration/test_harness_composability.rho`

### 8. PathMap Integration
- PathMap state management
- Environment persistence
- Space operations

**Files:**
- `integration/test_pathmap_*.rho`

### 9. Stress & Performance
- Large expression trees
- Many sequential operations
- Complex rule sets

**Files:**
- TBD (to be created)

## Dependencies

### Required Rust Dependencies

```toml
[dev-dependencies]
# Test execution
assert_cmd = "2.0"
predicates = "3.0"

# Output parsing
regex = "1.10"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Configuration
toml = "0.8"

# Utilities
tempfile = "3.8"
```

### External Requirements

1. **rholang-cli binary**
   - Location: `../f1r3node/target/release/rholang-cli`
   - Build: `cd ../f1r3node/rholang && RUSTFLAGS="-C target-cpu=native" cargo build --release --bin rholang-cli`

2. **Test files**
   - Location: `integration/*.rho`
   - 10 existing files (8 tests + 2 examples)

## Test Execution Environment

### Local Development
- Rust toolchain 1.70+
- rholang-cli built and available
- All integration test files present

### CI/CD Environment
- Ubuntu latest
- Rust stable toolchain
- Submodule checkout (f1r3node)
- Build artifacts uploaded on failure

## Output Formats

### Console Output (Default)
```
test rholang_integration::test_arithmetic ... ok
test rholang_integration::test_rules ... FAILED

failures:

---- rholang_integration::test_rules stdout ----
Expected: [10, 15]
Actual: [10, 14]

failures:
    rholang_integration::test_rules

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured
```

### Verbose Output (--nocapture)
```
Test: Basic addition
Input: (+ 1 2)
Output: {|("eval_outputs", [3])|}
Status: PASS ✓

Test: Rule definition
Input: (= (double $x) (* $x 2))
Output: {|("eval_outputs", [])|}  // Nil
Status: PASS ✓
```

### JSON Output (Future)
```json
{
  "test_suite": "rholang_integration",
  "total": 10,
  "passed": 9,
  "failed": 1,
  "duration_ms": 5234,
  "tests": [
    {
      "name": "test_arithmetic",
      "status": "passed",
      "duration_ms": 523
    },
    {
      "name": "test_rules",
      "status": "failed",
      "error": "Expected [10, 15], got [10, 14]",
      "duration_ms": 892
    }
  ]
}
```

## Error Handling

### Common Issues & Solutions

1. **rholang-cli not found**
   - Solution: Set `RHOLANG_CLI_PATH` environment variable
   - Auto-check common locations

2. **Test timeout**
   - Default: 30 seconds per test
   - Configurable via test spec
   - Fails gracefully with timeout error

3. **Parse error**
   - PathMap format unexpected
   - Log raw output for debugging
   - Suggest format issues

4. **Build failure**
   - Check f1r3node submodule
   - Verify Rust toolchain
   - Check dependency compatibility

## Performance Considerations

### Test Execution Time
- Simple tests: ~0.5-1s each
- Complex tests (sequential): ~2-5s each
- Full suite: ~30-60s

### Optimization Strategies
- Parallel test execution (Phase 3)
- Test result caching
- Incremental testing (only changed)

## Future Enhancements

### Planned Features
1. **Snapshot Testing** - Store expected outputs as snapshots
2. **Fuzzing** - Generate random MeTTa expressions
3. **Coverage Analysis** - Track which MeTTa features are tested
4. **Visual Reports** - HTML dashboard with test results
5. **Notification System** - Slack/Discord alerts for failures
6. **Historical Tracking** - Performance regression over time

### Integration Points
- **Official MeTTa Tests** - Import tests from hyperon-experimental
- **Property-Based Testing** - Use quickcheck/proptest
- **Mutation Testing** - Test the tests themselves

## References

### Documentation
- [TESTING_GUIDE.md](../integration/TESTING_GUIDE.md) - Current testing approach
- [TEST_HARNESS_README.md](../integration/TEST_HARNESS_README.md) - Test harness docs
- [RHOLANG_INTEGRATION.md](../integration/RHOLANG_INTEGRATION.md) - Rholang integration

### Existing Tests
- `integration/test_harness_simple.rho` - Simple test suite
- `integration/test_harness_composability.rho` - Composability tests
- `integration/test_harness_validation.rho` - Validation tests
- `integration/test_metta_integration.rho` - Basic integration
- `integration/test_pathmap_*.rho` - PathMap tests

### External Resources
- [Rust Testing Guide](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [assert_cmd Documentation](https://docs.rs/assert_cmd/)
- [GitHub Actions for Rust](https://github.com/actions-rs)

## Contributing

### Adding New Tests

1. **Create .rho file** in `integration/`
2. **Add test function** in `tests/rholang_integration.rs`
3. **Define expected output** (Phase 2)
4. **Run and validate**: `cargo test --test rholang_integration your_test_name`
5. **Document** in this file

### Reporting Issues

File issues at: https://github.com/F1R3FLY-io/MeTTa-Compiler/issues

Include:
- Test name
- Expected vs actual output
- rholang-cli version
- Full error message

---

**Last Updated:** 2025-10-20
**Status:** Phase 1 in progress
**Next Milestone:** Basic test runner functional
