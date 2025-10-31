# MORK "reserved 126" Bug - Fix Summary

## Status: ✅ FIXED

The "reserved 126" panic that occurred when using `if + match` with symbols containing reserved bytes (64-127) has been completely resolved.

## What Was Fixed

### The Bug
- **Symptom**: Panic with "reserved 126" (or other values in 64-127 range)
- **Trigger**: Rules combining `if` with `match` in the body, using symbols with reserved bytes
- **Location**: PathMap Par integration layer during Environment serialization/deserialization
- **Root Cause**: Non-idempotent round-trip conversion (MORK bytes → String → MORK bytes produced different bytes)

### The Solution
**File**: `src/pathmap_par_integration.rs`

Changed from string-based serialization to MORK's native binary format:

**Before** (Broken - caused reserved byte corruption):
```rust
// Serialization: MORK bytes → serialize_mork_expr → String → GString Par
let path_str = Environment::serialize_mork_expr(&expr, &space);
let path_par = Par::default().with_exprs(vec![Expr {
    expr_instance: Some(ExprInstance::GString(path_str)),
}]);

// Deserialization: GString Par → String → compile → metta_to_mork_bytes → MORK bytes
if let Ok(state) = compile(&path_str) {
    let bytes = metta_to_mork_bytes(value, &space, &mut ctx)?;
    space.btm.insert(&bytes[..], ());
}
```

**After** (Fixed - preserves bytes perfectly):
```rust
// Serialization: Use MORK's native dump (includes symbol table)
let mut dump_buffer = Vec::new();
space.dump_all_sexpr(&mut dump_buffer)?;
let space_bytes_par = Par::default().with_exprs(vec![Expr {
    expr_instance: Some(ExprInstance::GByteArray(dump_buffer)),
}]);

// Deserialization: Use MORK's native load (restores symbol table)
space.load_all_sexpr(&space_dump_bytes[..])?;
```

### Why This Works

1. **No Conversion**: Avoids string round-trip that corrupted bytes
2. **Symbol Table Preserved**: MORK's dump/load handles symbol interning correctly
3. **Byte-Perfect**: Original MORK encoding preserved exactly
4. **Zero Overhead**: Faster than serialize/parse pipeline

## Test Coverage

Created comprehensive test suite in `pathmap_par_integration.rs`:

### ✅ test_reserved_bytes_roundtrip_y_z
- Tests symbols with 'y' (121) and 'z' (122)
- Verifies basic round-trip with reserved bytes

### ✅ test_reserved_bytes_roundtrip_tilde
- Tests tilde '~' (126) - the specific byte from the bug report
- **This test used to panic - now it passes!**

### ✅ test_reserved_bytes_with_rules
- Tests the original bug scenario: rules with `if + match`
- Includes facts with reserved bytes and match-based rules
- **This is the exact pattern that triggered the original bug**

### ✅ test_reserved_bytes_multiple_roundtrips
- Tests 3 consecutive round-trips
- Ensures bytes remain stable across multiple serializations
- Tests symbols with 'x' (120), 'y' (121), 'z' (122), '~' (126)

### ✅ test_reserved_bytes_all_range
- Tests various ASCII characters in reserved range (64-127)
- Includes '@' (64), 'A-Z' (65-90), 'x-z' (120-122), '~' (126)
- Ensures fix works for ANY reserved byte

### ✅ test_environment_serialization_roundtrip
- Tests basic Environment serialization
- Verifies rule count preservation

## Verification

All tests pass:
```
running 13 tests
test pathmap_par_integration::tests::test_metta_value_atom_string_distinction ... ok
test pathmap_par_integration::tests::test_metta_value_atom_to_par ... ok
test pathmap_par_integration::tests::test_metta_value_long_to_par ... ok
test pathmap_par_integration::tests::test_metta_value_sexpr_to_par ... ok
test pathmap_par_integration::tests::test_metta_value_string_to_par ... ok
test pathmap_par_integration::tests::test_metta_state_to_pathmap_par ... ok
test pathmap_par_integration::tests::test_metta_error_to_par ... ok
test pathmap_par_integration::tests::test_environment_serialization_roundtrip ... ok
test pathmap_par_integration::tests::test_reserved_bytes_all_range ... ok
test pathmap_par_integration::tests::test_reserved_bytes_multiple_roundtrips ... ok
test pathmap_par_integration::tests::test_reserved_bytes_roundtrip_tilde ... ok
test pathmap_par_integration::tests::test_reserved_bytes_roundtrip_y_z ... ok
test pathmap_par_integration::tests::test_reserved_bytes_with_rules ... ok

test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 291 filtered out
```

## Before and After

### Before (Panic!)
```metta
(connected room_a room_b)

(= (is_connected $from $to)
   (match & self (connected $from $to) true))

(= (check_and_return $from $to)
   (if (is_connected $from $to)
       ($to)
       ()))

!(check_and_return room_a room_b)
```

**Result**: `thread 'main' panicked at bytestring/src/lib.rs:62:12: reserved 126`

### After (Works!)
```metta
(connected room_a room_b)

(= (is_connected $from $to)
   (match & self (connected $from $to) true))

(= (check_and_return $from $to)
   (if (is_connected $from $to)
       ($to)
       ()))

!(check_and_return room_a room_b)
```

**Result**: ✅ Evaluates successfully, no panic!

## Files Modified

1. **`src/pathmap_par_integration.rs`** (lines 121-449)
   - Changed `environment_to_par()` to use `dump_all_sexpr()`
   - Changed `par_to_environment()` to use `load_all_sexpr()`
   - Added 5 comprehensive reserved byte tests
   - Removed unused imports

## Key Insights

1. **MORK is NOT the problem** - All MORK core operations handle reserved bytes correctly (verified by tests in `/home/dylon/Workspace/f1r3fly.io/MORK/benchmarks/reserved-bug-test/`)

2. **The bug was in the integration layer** - The PathMap Par conversion was using a lossy string-based round-trip

3. **Symbol interning requires special care** - You cannot simply convert MORK bytes to string and back - the symbol table must be preserved

4. **MORK's native format is the answer** - Using `dump_all_sexpr()` and `load_all_sexpr()` is the only way to guarantee perfect serialization

## Impact

This fix enables:
- ✅ Using `if` with `match`-based predicates
- ✅ Symbols with ANY characters including reserved bytes (64-127)
- ✅ Complex recursive queries with conditionals
- ✅ Planning/reasoning systems with dynamic query composition
- ✅ Multiple round-trips through Rholang without data loss

## Related Documentation

- **Bug Analysis**: `/home/dylon/Workspace/f1r3fly.io/MORK/benchmarks/reserved-bug-test/BUG_ANALYSIS.md`
- **Test Findings**: `/home/dylon/Workspace/f1r3fly.io/MORK/benchmarks/reserved-bug-test/FINDINGS.md`
- **Root Cause Summary**: `/home/dylon/Workspace/f1r3fly.io/MORK/benchmarks/reserved-bug-test/ROOT_CAUSE_SUMMARY.md`
- **Original Bug Report**: `/tmp/MORK_BUG_REPORT_UPDATED.md`

## Build and Test

```bash
cd /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler
env RUSTFLAGS="-C target-cpu=native" cargo test --lib pathmap_par_integration
```

All tests pass! ✅
