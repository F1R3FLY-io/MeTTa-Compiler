# MeTTaTron Test Harness - Quick Start Guide

## 🚀 Quick Start

### Run Tests (5 seconds)

```bash
cd /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration

# Simple tests (10 tests, ~30 seconds)
/home/dylon/Workspace/f1r3fly.io/f1r3node/target/release/rholang-cli test_harness_simple.rho

# Validation tests (20+ tests, ~60 seconds)
/home/dylon/Workspace/f1r3fly.io/f1r3node/target/release/rholang-cli test_harness_validation.rho

# Full suite (15 tests with advanced patterns, ~45 seconds)
/home/dylon/Workspace/f1r3fly.io/f1r3node/target/release/rholang-cli test_harness.rho
```

### View Results

```bash
# Filter for test names
rholang-cli test_harness_simple.rho 2>&1 | grep "Test:"

# Show test results only
rholang-cli test_harness_simple.rho 2>&1 | grep -A2 "Result:"

# Save full output
rholang-cli test_harness_validation.rho > results.txt 2>&1
```

## 📝 Add Your Own Test (30 seconds)

Edit `test_harness_simple.rho`, add before the "Final message" section:

```rholang
// Your test here
for (_ <- ack) {
  testHarness!("My test name", "(+ 100 200)")
} |
```

Run it:
```bash
rholang-cli test_harness_simple.rho
```

## 🔧 Core Contracts

### `runMeTTa(ret, code)` - Compile and Evaluate

```rholang
new result in {
  runMeTTa!(*result, "(+ 1 2)") |
  for (@state <- result) {
    // state is PathMap: {|("pending_exprs", []), ("environment", ...), ("eval_outputs", ["3"])|}
  }
}
```

### `testHarness(name, code)` - Test Runner

```rholang
testHarness!("Addition test", "(+ 1 2)")
// Outputs:
// Test: Addition test
//   Code: (+ 1 2)
//   Result: {|..., ("eval_outputs", ["3"])|}
```

## 🔗 Sequential Tests (Rules)

```rholang
// Step 1: Define rule
runMeTTa!(*result1, "(= (double $x) (* $x 2))") |
for (@acc1 <- result1) {
  // Step 2: Use rule
  mettaCompile!("!(double 21)", *useCompiled) |
  for (@compiled <- useCompiled) {
    new result2 in {
      result2!(acc1.run(compiled)) |
      for (@acc2 <- result2) {
        // acc2 eval_outputs: ["Nil", "42"]
      }
    }
  }
}
```

## 📊 What Tests Cover

| Feature | Example |
|---------|---------|
| Arithmetic | `(+ 1 2)` → `3` |
| Comparison | `(< 1 2)` → `true` |
| Conditional | `(if (< 1 2) "yes" "no")` → `"yes"` |
| Quote | `(quote (+ 1 2))` → `(+ 1 2)` |
| Eval | `(eval (quote (+ 1 2)))` → `3` |
| Error | `(error "msg" 42)` |
| Catch | `(catch (error "e" 0) "ok")` → `"ok"` |
| Rule def | `(= (double $x) (* $x 2))` → `Nil` |
| Rule use | `!(double 21)` → `42` |

## 🐛 Troubleshooting

**Environment shows as `({||}, [])`**
- This is normal - binary paths aren't displayed
- Functional tests confirm data is present
- Rules work correctly after serialization

**Test hangs**
- Check ack chain: `for (_ <- ack)`
- Ensure all sends have corresponding receives

**Unexpected results**
- Add debug output: `stdoutAck!(intermediateState, *ack)`
- Check rule was defined (returns `Nil`)
- Verify accumulated state passed correctly

## 📚 Documentation

- **Full Guide**: `TEST_HARNESS_README.md`
- **Implementation**: `TEST_HARNESS_SUMMARY.md`
- **Integration**: `RHOLANG_INTEGRATION.md`

## 🎯 Common Tasks

### Test a Single MeTTa Expression

```rholang
testHarness!("My test", "(* 3 7)")
```

### Test Multiple Expressions

```rholang
for (_ <- ack) {
  testHarness!("Test 1", "(+ 1 2)")
} |
for (_ <- ack) {
  testHarness!("Test 2", "(* 3 4)")
}
```

### Test Rule Definition and Usage

Use the sequential pattern shown above in "Sequential Tests (Rules)".

---

**Total Setup Time**: < 1 minute
**Time to First Test**: < 30 seconds
**Time to Custom Test**: < 2 minutes

Happy Testing! 🎉
