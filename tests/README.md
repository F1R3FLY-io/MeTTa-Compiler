# Integration Tests

Automated test suite for Rholang integration with MeTTa.

## Quick Start

### Run All Tests
```bash
cargo test --test rholang_integration
```

### Run Specific Test
```bash
cargo test --test rholang_integration test_arithmetic
```

### Run with Verbose Output
```bash
cargo test --test rholang_integration -- --nocapture --test-threads=1
```

### Run Ignored Tests (Stress/Performance)
```bash
cargo test --test rholang_integration -- --ignored
```

## Prerequisites

### 1. Build rholang-cli

```bash
cd ../f1r3node/rholang
RUSTFLAGS="-C target-cpu=native" cargo build --release --bin rholang-cli
```

The binary will be at: `../f1r3node/target/release/rholang-cli`

### 2. Set Environment Variable (Optional)

If rholang-cli is in a non-standard location:

```bash
export RHOLANG_CLI_PATH=/custom/path/to/rholang-cli
cargo test --test rholang_integration
```

## Test Structure

```
tests/
├── README.md                    # This file
├── rholang_integration.rs       # Main test suite
└── utils.rs                     # Test utilities
```

## Test Categories

### Basic Integration Tests
- `test_metta_integration` - Basic MeTTa/Rholang integration
- `test_pathmap_simple` - Simple PathMap operations
- `test_pathmap_state` - PathMap state management
- `test_pathmap_run_method` - PathMap run method

### Test Harness Tests
- `test_harness_simple` - Simple test harness suite
- `test_harness_composability` - Composability tests
- `test_harness_validation` - Validation tests

### Feature Tests
- `test_arithmetic_operations` - Arithmetic: +, -, *, /
- `test_rule_definitions` - Rule definition and usage
- `test_error_handling` - Error handling: error, catch, is-error

### Examples
- `test_example_robot_planning` - Robot planning example
- `test_example_metta_rholang` - Basic MeTTa/Rholang example

### Stress Tests (Ignored by Default)
- `test_stress_sequential_operations` - Sequential operation stress test

## Test Output

### Success
```
running 15 tests
test test_metta_integration ... ok
test test_pathmap_simple ... ok
test test_harness_simple ... ok
...
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured
```

### Failure
```
running 15 tests
test test_harness_simple ... FAILED

failures:

---- test_harness_simple stdout ----
thread 'test_harness_simple' panicked at 'Test failed: test_harness_simple.rho'

=== STDOUT ===
Error: Expected 3, got 4
...

failures:
    test_harness_simple

test result: FAILED. 14 passed; 1 failed; 0 ignored; 0 measured
```

## Troubleshooting

### Error: "rholang-cli not found"

**Solution 1:** Build rholang-cli
```bash
cd ../f1r3node/rholang
RUSTFLAGS="-C target-cpu=native" cargo build --release --bin rholang-cli
```

**Solution 2:** Set environment variable
```bash
export RHOLANG_CLI_PATH=/path/to/rholang-cli
```

**Solution 3:** Verify path
```bash
ls -la ../f1r3node/target/release/rholang-cli
```

### Error: "Test file not found"

Check that integration test files exist:
```bash
ls -la integration/*.rho
```

Expected files:
- `test_metta_integration.rho`
- `test_pathmap_simple.rho`
- `test_pathmap_state.rho`
- `test_pathmap_run_method.rho`
- `test_harness_simple.rho`
- `test_harness_composability.rho`
- `test_harness_validation.rho`

### Test Hangs or Times Out

Tests may hang if:
1. Rholang file has infinite loop
2. Missing ack channel in Rholang
3. Deadlock in state management

Use `--test-threads=1` to run tests sequentially for easier debugging:
```bash
cargo test --test rholang_integration -- --nocapture --test-threads=1
```

### Verbose Debugging

See full output including stdout from tests:
```bash
RUST_LOG=debug cargo test --test rholang_integration -- --nocapture
```

## Adding New Tests

### 1. Create Test Function

In `rholang_integration.rs`:

```rust
#[test]
fn test_my_feature() {
    let (success, stdout, stderr) = run_rho_test("test_my_feature.rho");

    assert!(success, "Test failed to execute");

    // Add your validations
    assert!(stdout.contains("expected output"));
}
```

### 2. Create Test File

Create `integration/test_my_feature.rho`:

```rholang
new stdoutAck(`rho:io:stdoutAck`),
    mettaCompile(`rho:metta:compile`),
    ack in {

  stdoutAck!("Testing my feature...\n", *ack) |
  for (_ <- ack) {
    // Your test code here
    stdoutAck!("Test complete!\n", *ack)
  }
}
```

### 3. Run Test

```bash
cargo test --test rholang_integration test_my_feature
```

## Continuous Integration

See `.github/workflows/integration-tests.yml` for CI configuration (coming in Phase 4).

## Future Enhancements

- [ ] Output validation with expected values
- [ ] Test timeouts (currently unlimited)
- [ ] Parallel test execution
- [ ] JSON output parsing
- [ ] Performance benchmarking
- [ ] Test result reporting (HTML/JSON)

## References

- [Integration Testing Implementation](../docs/INTEGRATION_TESTING_IMPLEMENTATION.md)
- [Testing Guide](../integration/TESTING_GUIDE.md)
- [Test Harness README](../integration/TEST_HARNESS_README.md)
