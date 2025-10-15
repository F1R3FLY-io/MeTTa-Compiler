# MeTTaTron Test Harness

Comprehensive test suite for MeTTa compiler and Rholang integration.

## Test Files

### 1. `test_harness_simple.rho`
**Purpose**: Minimal, easy-to-understand test harness
**Use Case**: Quick testing, debugging, learning the API

**Features**:
- Simple `runMeTTa` contract for compile + evaluate
- Basic `testHarness` contract for display
- 10 core test cases covering:
  - Basic arithmetic (`+`, `-`, `*`, `/`)
  - Comparisons (`<`, `>`, `==`)
  - Rule definition and usage
  - Error handling (`error`, `catch`)
  - Quote/Eval

**Usage**:
```bash
/path/to/rholang-cli test_harness_simple.rho
```

### 2. `test_harness_validation.rho`
**Purpose**: Test harness with expected output validation
**Use Case**: Regression testing, CI/CD integration

**Features**:
- Expected output specification
- Organized test categories:
  - Arithmetic Tests (5 tests)
  - Boolean Tests (5 tests)
  - Control Flow Tests (2 tests)
  - Quote/Eval Tests (2 tests)
  - Error Handling Tests (4 tests)
  - Rule Tests (2 tests)
  - Stress Test (REPL simulation)
- Output validation placeholders

**Usage**:
```bash
/path/to/rholang-cli test_harness_validation.rho
```

### 3. `test_harness.rho`
**Purpose**: Full-featured test harness with advanced patterns
**Use Case**: Comprehensive testing, complex scenarios

**Features**:
- Multiple-expression handling
- Sequential state accumulation
- Rule chaining tests
- REPL simulation (3 sequential inputs)
- 15 test cases covering all MeTTa features

**Usage**:
```bash
/path/to/rholang-cli test_harness.rho
```

## Test Harness Architecture

### Core Contracts

#### `runMeTTa(ret, code)`
Compiles and evaluates a single MeTTa expression.

```rholang
contract runMeTTa(@ret, @code) = {
  new compiledState in {
    mettaCompile!(code, *compiledState) |
    for (@pm <- compiledState) {
      ret!({||}.run(pm))
    }
  }
}
```

**Parameters**:
- `ret`: Return channel for accumulated state
- `code`: MeTTa source code string

**Returns**: PathMap with structure:
```
{|
  ("pending_exprs", []),
  ("environment", ({||}, [])),
  ("eval_outputs", [result1, result2, ...])
|}
```

#### `testHarness(testName, code)`
Runs a test and displays results.

```rholang
contract testHarness(@testName, @code) = {
  new result in {
    stdoutAck!("Test: " ++ testName ++ "\n", *ack) |
    for (_ <- ack) {
      runMeTTa!(*result, code) |
      for (@rslt <- result) {
        stdoutAck!("  Result: ", *ack) |
        for (_ <- ack) {
          stdoutAck!(rslt, *ack)
        }
      }
    }
  }
}
```

### Sequential State Accumulation Pattern

For tests requiring rule definitions and usage:

```rholang
// Step 1: Define rule
runMeTTa!(*result1, "(= (double $x) (* $x 2))") |
for (@acc1 <- result1) {
  // Step 2: Use rule with accumulated state
  mettaCompile!("!(double 21)", *useCompiled) |
  for (@compiled <- useCompiled) {
    result2!(acc1.run(compiled)) |
    for (@acc2 <- result2) {
      // acc2 now contains both rule definition and usage result
    }
  }
}
```

## Test Coverage

### Arithmetic Operations
- Addition: `(+ 1 2)` → `3`
- Subtraction: `(- 10 5)` → `5`
- Multiplication: `(* 3 4)` → `12`
- Division: `(/ 10 2)` → `5`
- Nested: `(+ 1 (* 2 3))` → `7`

### Boolean Operations
- Less than: `(< 1 2)` → `true`
- Greater than: `(> 5 3)` → `true`
- Equal: `(== 4 4)` → `true`
- Less or equal: `(<= 2 2)` → `true`

### Control Flow
- Conditional (true): `(if (< 1 2) "yes" "no")` → `"yes"`
- Conditional (false): `(if (> 1 2) "yes" "no")` → `"no"`

### Quote and Eval
- Quote: `(quote (+ 1 2))` → `(+ 1 2)` (unevaluated)
- Eval: `(eval (quote (+ 1 2)))` → `3`

### Error Handling
- Error creation: `(error "test" 42)`
- Catch: `(catch (error "e" 0) "recovered")` → `"recovered"`
- Is-error (true): `(is-error (error "e" 0))` → `true`
- Is-error (false): `(is-error 42)` → `false`

### Rules
- Rule definition: `(= (double $x) (* $x 2))` → `Nil`
- Rule usage: `!(double 21)` → `42` (after defining rule)
- Rule chaining: `(= (quadruple $x) (double (double $x)))`

## Output Format

Each test produces a PathMap result:

```
{|
  ("pending_exprs", []),           // Empty after evaluation
  ("environment", ({||}, [])),     // Space (as EPathMap) and types
  ("eval_outputs", [results...])   // Accumulated evaluation results
|}
```

**Note**: The environment's Space EPathMap may display as `{||}` due to binary path encoding, but functional tests confirm data is present.

## Running Tests

### Basic Run
```bash
cd /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration
/home/dylon/Workspace/f1r3fly.io/f1r3node/target/release/rholang-cli test_harness_simple.rho
```

### Capture Output
```bash
/path/to/rholang-cli test_harness_validation.rho > test_results.txt 2>&1
```

### Filter for Test Results
```bash
/path/to/rholang-cli test_harness_simple.rho 2>&1 | grep -A2 "Test:"
```

## Extending the Test Harness

### Adding a New Test

```rholang
// In the test suite section, add:
for (_ <- ack) {
  testHarness!("Your test name", "(your-metta-code)")
}
```

### Adding Sequential State Tests

```rholang
for (_ <- ack) {
  stdoutAck!("Test: Your sequential test\n", *ack) |
  for (_ <- ack) {
    new result1 in {
      runMeTTa!(*result1, "(first-operation)") |
      for (@acc1 <- result1) {
        new useCompiled, result2 in {
          mettaCompile!("(second-operation)", *useCompiled) |
          for (@compiled <- useCompiled) {
            result2!(acc1.run(compiled)) |
            for (@acc2 <- result2) {
              stdoutAck!(acc2, *ack)
            }
          }
        }
      }
    }
  }
}
```

### Adding Validation Logic

To validate specific outputs:

```rholang
for (@rslt <- result) {
  match rslt {
    pathmap => {
      // Extract eval_outputs
      match pathmap.get("eval_outputs") {
        outputs => {
          // Validate output count, values, etc.
          if (outputs.length() == expectedCount) {
            stdoutAck!("✓ Pass\n", *ack)
          } else {
            stdoutAck!("✗ Fail\n", *ack)
          }
        }
      }
    }
  }
}
```

## Expected Behavior

### Successful Test Output
```
Test: Basic addition
  Code: (+ 1 2)
  Result: {|("pending_exprs", []), ("environment", ({||}, [])), ("eval_outputs", ["3"])|}
```

### Rule Test Output
```
Test: Rule definition and usage (sequential)
  Step 1 (define): {|..., ("eval_outputs", ["Nil"])|}
  Step 2 (use): {|..., ("eval_outputs", ["Nil", "42"])|}
```

## Troubleshooting

### Empty Environment Display
**Issue**: Environment shows as `{||}`
**Explanation**: Binary PathMap keys (MORK paths) aren't displayed in human-readable form
**Solution**: This is expected behavior. Functional tests confirm data is present.

### Test Hangs
**Issue**: Test doesn't complete
**Cause**: Missing ack channel or circular dependency
**Solution**: Ensure all `for (_ <- ack)` chains are properly sequenced

### Incorrect Results
**Issue**: Unexpected eval_outputs
**Debugging**:
1. Check rule definition step succeeded (returned Nil)
2. Verify accumulated state passed correctly to next operation
3. Add intermediate stdoutAck calls to trace execution

## Performance Notes

- Each test creates new channels and contracts
- Sequential tests may take longer due to state accumulation
- Stress tests (10+ operations) may produce verbose output
- Consider running subsets of tests for quick validation

## Future Enhancements

1. **Automated Validation**: Parse PathMap output and validate results programmatically
2. **Test Isolation**: Reset state between tests for independence
3. **Performance Metrics**: Measure compilation and evaluation time
4. **Error Recovery**: Graceful handling of test failures
5. **Parallel Execution**: Run independent tests concurrently
6. **Test Filtering**: Command-line arguments to select specific tests
7. **Output Formatting**: Pretty-print PathMap results

## References

- MeTTa Language Documentation: `docs/reference/METTA_TYPE_SYSTEM_REFERENCE.md`
- Rholang Integration: `integration/RHOLANG_INTEGRATION.md`
- PathMap API: `docs/design/PATHMAP_STATE_DESIGN.md`
- Test Examples: `integration/test_pathmap_run_method.rho`
