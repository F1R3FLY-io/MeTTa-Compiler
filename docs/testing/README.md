# Testing Documentation

Documentation for testing strategies, frameworks, and implementation in MeTTaTron.

## Integration Testing

**[INTEGRATION_TESTING_IMPLEMENTATION.md](INTEGRATION_TESTING_IMPLEMENTATION.md)** - Integration Test Framework
- Test framework architecture
- Integration test structure
- Test harness implementation
- Testing Rholang integration
- Running integration tests
- Test file organization
- Best practices for writing tests
- Debugging failed tests

## Running Tests

### All Tests
```bash
cargo test
```

### With Output
```bash
cargo test -- --nocapture
```

### Specific Test
```bash
cargo test test_compile_simple
```

### Integration Tests Only
```bash
cargo test --test '*'
```

## Test Categories

MeTTaTron has 474 total tests distributed across:

- **Library tests** (385 tests) - Core functionality in `src/`
- **Binary tests** (13 tests) - CLI and binary features
- **Integration tests** (69 tests) - Rholang integration in `integration/`
- **Other tests** (7 tests) - Additional test suites

## Additional Testing Resources

- **`integration/TESTING_GUIDE.md`** - Comprehensive testing guide for Rholang integration
- **`integration/TEST_HARNESS_README.md`** - Test harness documentation
- **`tests/QUERY_LANGUAGE.md`** - Query language testing documentation

## Testing Best Practices

1. **Write tests for new features** - Every new feature should have corresponding tests
2. **Test edge cases** - Consider boundary conditions and error cases
3. **Use descriptive test names** - Test names should clearly indicate what is being tested
4. **Keep tests focused** - Each test should verify one specific behavior
5. **Run tests frequently** - Run tests before committing changes
6. **Fix failing tests immediately** - Don't let broken tests accumulate
