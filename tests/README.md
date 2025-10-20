# Integration Tests

Automated test suite for Rholang integration with MeTTa and standalone mettatron binary testing.

## Quick Start

### Run All Tests
```bash
cargo test
```

### Run Specific Test Suite
```bash
# Rholang integration tests
cargo test --test rholang_integration

# Mettatron binary tests
cargo test --test mettatron_binary
```

### Run Specific Test
```bash
cargo test --test rholang_integration test_arithmetic_operations
```

### Run with Verbose Output
```bash
cargo test --test rholang_integration -- --nocapture --test-threads=1
```

## Prerequisites

### 1. Build mettatron
```bash
cargo build --release
```

The binary will be at: `./target/release/mettatron`

**Note:** Build flags are configured automatically via `.cargo/config.toml`. No manual RUSTFLAGS setup required.

### 2. Build rholang-cli (for Rholang integration tests)
```bash
cd ../f1r3node/rholang
cargo build --release --bin rholang-cli
```

The binary will be at: `../f1r3node/target/release/rholang-cli`

### 3. Set Environment Variable (Optional)

If rholang-cli is in a non-standard location:

```bash
export RHOLANG_CLI_PATH=/custom/path/to/rholang-cli
cargo test --test rholang_integration
```

## Test Structure

```
tests/
├── README.md                    # This file
├── rholang_integration.rs       # Rholang integration tests
├── mettatron_binary.rs          # Mettatron binary tests
├── utils.rs                     # Test utilities and module exports
├── output_parser.rs             # PathMap output parsing (Phase 2)
├── test_specs.rs                # Test specification structures (Phase 2)
└── validators.rs                # Output validation logic (Phase 2)
```

## Test Suites

### Rholang Integration Tests (17 tests)

Tests MeTTa-Rholang integration via rholang-cli.

#### Basic Integration
- `test_metta_integration` - Basic MeTTa/Rholang integration
- `test_pathmap_simple` - Simple PathMap operations
- `test_pathmap_state` - PathMap state management
- `test_pathmap_run_method` - PathMap run method

#### Test Harness
- `test_harness_simple` - Simple test harness suite with arithmetic validation
- `test_harness_composability` - Composability tests with rule validation
- `test_harness_validation` - Validation tests

#### Examples
- `test_example_robot_planning` - Robot planning example
- `test_example_metta_rholang` - Basic MeTTa/Rholang example

#### Utility Tests
- `test_rholang_cli_exists` - Verify rholang-cli binary exists
- `test_integration_dir_exists` - Verify integration directory exists
- `test_all_test_files_exist` - Verify all test files are present

### Mettatron Binary Tests (13 tests)

Tests the standalone mettatron executable.

#### Binary Verification
- `test_binary_exists` - Binary exists and is executable
- `test_binary_runs` - Binary executes with --help

#### Example Evaluation
- `test_evaluate_simple_metta` - Evaluate simple.metta
- `test_evaluate_advanced_metta` - Evaluate advanced.metta
- `test_evaluate_mvp_test` - Evaluate mvp_test.metta
- `test_evaluate_type_system_demo` - Evaluate type_system_demo.metta
- `test_evaluate_pathmap_demo` - Evaluate pathmap_demo.metta

#### Command-Line Options
- `test_sexpr_option` - --sexpr flag (S-expression output)
- `test_output_to_file` - -o flag (file output)
- `test_stdin_input` - - flag (stdin input)

#### Error Handling
- `test_nonexistent_file` - Nonexistent file error
- `test_invalid_metta_syntax` - Invalid syntax handling

#### Comprehensive
- `test_all_metta_examples` - All .metta files in examples/

## Phase 2: Output Validation (NEW!)

Phase 2 introduces structured output validation capabilities:

### PathMap Structure

PathMap output from Rholang follows this structure:

```
{|
  ("source", [expr1, expr2, ...]),           // Formerly "pending_exprs"
  ("environment", ({||...||}, [...])),       // Space state
  ("output", [result1, result2, ...])        // Formerly "eval_outputs"
|}
```

### Validation Features

#### 1. PathMap Parsing
```rust
use crate::utils::{parse_pathmap, PathMapOutput};

let output = r#"{|("source", [(+ 1 2)]), ("output", [3])|}"#;
let pathmaps = parse_pathmap(output);
assert_eq!(pathmaps[0].output, vec!["3"]);
```

#### 2. Test Specifications
```rust
use crate::utils::{RholangTestSpec, Expectation};

let spec = RholangTestSpec::new("arithmetic_test", "test.rho")
    .with_timeout(60)
    .expect(Expectation::outputs("addition works", vec!["3".to_string()]))
    .expect(Expectation::no_errors("no errors"))
    .expect(Expectation::success("exits cleanly"));
```

#### 3. Output Validation
```rust
use crate::utils::validate;

let result = validate(stdout, stderr, exit_code, &expectation);
if result.is_pass() {
    println!("✓ Test passed!");
} else {
    println!("✗ Test failed: {}", result.failure_reason().unwrap());
}
```

### Validation Patterns

- **Contains**: Check if stdout contains specific text
- **Regex**: Match stdout against regex pattern
- **Outputs**: Validate specific values in PathMap "output" field
- **PathMapStructure**: Validate PathMap has expected structure
- **NoErrors**: Ensure no error indicators in output
- **Success**: Ensure exit code is 0
- **Custom**: Custom validation function

### Test Reports

```rust
use crate::utils::TestReport;

let mut report = TestReport::new("my_test");
report.add_result("expectation 1", ValidationResult::pass());
report.add_result("expectation 2", ValidationResult::fail("mismatch"));

println!("{}", report.format());
// Outputs:
// === Test Report: my_test ===
// Status: FAILED ✗
// Results: 1 passed, 1 failed
// Duration: 123ms
//
// Expectations:
//   ✓ expectation 1
//   ✗ expectation 2
//     Reason: mismatch
```

## Test Output

### Success
```
running 30 tests
test test_metta_integration ... ok
test test_pathmap_simple ... ok
test test_binary_exists ... ok
...
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured
```

### Failure
```
running 30 tests
test test_harness_simple ... FAILED

failures:

---- test_harness_simple stdout ----
thread 'test_harness_simple' panicked at 'Test failed: test_harness_simple.rho'

=== STDOUT ===
Expected: [3, 7]
Actual: [3, 8]
...

failures:
    test_harness_simple

test result: FAILED. 29 passed; 1 failed; 0 ignored; 0 measured
```

## Troubleshooting

### Error: "mettatron binary not found"

**Solution:** Build mettatron
```bash
cargo build --release
```

### Error: "rholang-cli not found"

**Solution 1:** Build rholang-cli
```bash
cd ../f1r3node/rholang
cargo build --release --bin rholang-cli
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

    // Phase 2: Use structured validation
    let pathmaps = parse_pathmap(&stdout);
    assert!(!pathmaps.is_empty(), "No PathMap found");

    // Validate outputs
    assert_eq!(pathmaps[0].output, vec!["expected"]);

    // Or use expectations
    let expectation = Expectation::outputs("check result", vec!["expected".to_string()]);
    let result = validate(&stdout, &stderr, 0, &expectation);
    assert!(result.is_pass());
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
    for (@result <- mettaCompile!?("(+ 1 2)")) {
      stdoutAck!(result, *ack)
    }
  }
}
```

### 3. Run Test

```bash
cargo test --test rholang_integration test_my_feature
```

## Continuous Integration

See `.github/workflows/integration-tests.yml` for CI configuration (coming in Phase 4).

## Implementation Status

- ✅ **Phase 1 Complete**: Basic test runner
- ✅ **Phase 2 Complete**: Output validation with PathMap parsing
- ⏳ **Phase 3**: Test configuration & organization
- ⏳ **Phase 4**: CI/CD integration
- ⏳ **Phase 5**: Enhanced features (benchmarking, HTML reports)

## References

- [Integration Testing Implementation](../docs/INTEGRATION_TESTING_IMPLEMENTATION.md) - Full implementation plan
- [Testing Guide](../integration/TESTING_GUIDE.md) - Testing approach
- [Test Harness README](../integration/TEST_HARNESS_README.md) - Test harness documentation
- [Rholang Integration](../integration/RHOLANG_INTEGRATION.md) - Rholang integration details
