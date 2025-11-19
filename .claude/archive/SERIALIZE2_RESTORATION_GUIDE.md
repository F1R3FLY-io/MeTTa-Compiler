# serialize2() Restoration Guide

## Status: WORKAROUNDS IN PLACE

This document explains the serialize2() bug and provides instructions for restoring serialize2() usage once the MORK team fixes the underlying issues.

## The Bug

### Root Cause

MORK's bytestring format uses a 2-bit prefix tagging scheme:
- `0b00xxxxxx` (0-63): Arity tags
- `0b10xxxxxx` (128-191): VarRef tags
- `0b11xxxxxx` (192-255): NewVar/SymbolSize tags
- `0b01xxxxxx` (64-127): **RESERVED** - panic if encountered as tag

**The Problem**: `serialize2()` uses `byte_item()` which panics when it reads a reserved byte (64-127) in a position where it expects a tag. When symbols contain characters like 'o' (111), 'n' (110), 'r' (114), etc., these bytes can appear in the MORK representation and trigger the panic.

### Symptoms

- Panic messages: `"reserved 126"`, `"reserved 111"`, `"reserved 114"`, etc.
- Occurs during:
  - Environment serialization (PathMap Par integration)
  - Rule iteration and pattern matching
  - Type checking operations
  - Any operation that converts MORK Expr to text

### Why It Happens

MORK's symbol interning stores symbols as compact IDs rather than full strings. When `serialize2()` tries to convert these binary representations to text, it interprets bytes as tags. If a byte in the symbol data is in the reserved range (64-127), `byte_item()` panics because it treats it as an invalid tag byte.

## Current Workarounds

We have implemented workarounds in three main areas:

### 1. PathMap Par Integration (`src/pathmap_par_integration.rs`)

**Problem**: `dump_all_sexpr()` calls `serialize2()` which panics on reserved bytes

**Workaround** (lines 130-259):
```rust
// OLD (BROKEN): Use dump_all_sexpr()
space.dump_all_sexpr(&mut dump_buffer)?;

// NEW (WORKAROUND): Collect raw path bytes directly
let mut all_paths_data = Vec::new();
let mut rz = space.btm.read_zipper();

// Serialize symbol table
let symbol_table_bytes = { /* backup symbols to temp file */ };
all_paths_data.extend_from_slice(&sym_len.to_be_bytes());
all_paths_data.extend_from_slice(&symbol_table_bytes);

// Collect raw path bytes
while rz.to_next_val() {
    let path_bytes = rz.path();
    let len = path_bytes.len() as u32;
    all_paths_data.extend_from_slice(&len.to_be_bytes());
    all_paths_data.extend_from_slice(path_bytes);
}
```

**Affected Functions**:
- `environment_to_par()` (lines 128-259) - Serialization
- `par_to_environment()` (lines 448-642) - Deserialization

### 2. MORK Expr to MettaValue Conversion (`src/backend/types.rs`)

**Problem**: `serialize_mork_expr()` calls `serialize2()` which panics on reserved bytes

**Workaround** (lines 76-216):
```rust
// OLD (BROKEN): Use serialize2()
fn serialize_mork_expr_old(expr: &Expr, space: &Space) -> String {
    let mut buffer = Vec::new();
    expr.serialize2(&mut buffer, /* callbacks */);
    String::from_utf8_lossy(&buffer).to_string()
}

// NEW (WORKAROUND): Use maybe_byte_item() directly
pub(crate) fn mork_expr_to_metta_value(expr: &Expr, space: &Space) -> Result<MettaValue, String> {
    // Read byte and interpret as tag using maybe_byte_item (doesn't panic!)
    let byte = unsafe { *ptr.byte_add(offset) };
    let tag = match maybe_byte_item(byte) {
        Ok(t) => t,
        Err(reserved_byte) => {
            // Return error instead of panicking
            return Err(format!("Reserved byte {} at offset {}", reserved_byte, offset));
        }
    };
    // ... build MettaValue directly from tags
}
```

**Affected Functions**:
- `get_type()` (line 200) - Type assertions lookup
- `iter_rules()` (line 244) - Rule iteration
- `match_space()` (line 291) - Pattern matching
- `has_sexpr_fact()` (line 397) - Fact existence checking

**Deprecated Function**:
- `serialize_mork_expr_old()` (lines 150-170) - Kept for reference

### 3. MORK Bindings Conversion (`src/backend/mork_convert.rs`)

**Problem**: Converting pattern match bindings calls `serialize2()` indirectly

**Workaround** (lines 178-204):
```rust
// OLD (BROKEN): Use serialize2-based conversion
let value = Environment::serialize_mork_expr(&expr, space);

// NEW (WORKAROUND): Use mork_expr_to_metta_value()
let expr = expr_env.subsexpr();
if let Ok(value) = Environment::mork_expr_to_metta_value(&expr, space) {
    bindings.insert(format!("${}", var_name), value);
}
```

**Affected Functions**:
- `mork_bindings_to_metta()` (lines 178-204) - Pattern match bindings conversion

## How to Restore serialize2() Once MORK Fixes the Bug

### Prerequisites

Before restoring serialize2(), verify the MORK team has fixed:
1. `byte_item()` to handle reserved bytes gracefully (return error instead of panic)
2. `serialize2()` to properly handle symbol interning with reserved bytes
3. `dump_all_sexpr()` to not panic on reserved bytes

### Test Before Restoring

Create a test file with reserved bytes:
```metta
(connected room_a room_b)
(object_at robot room_a)

(= (is_connected $from $to)
   (match & self (connected $from $to) true))

!(is_connected room_a room_b)
```

Verify in MORK repository:
```bash
cd /path/to/MORK
# Test that serialize2 doesn't panic
cargo test --lib bytestring::tests::test_reserved_bytes
# Test that dump_all_sexpr doesn't panic
cargo test --lib space::tests::test_dump_reserved_bytes
```

### Restoration Steps

#### Step 1: Restore PathMap Par Integration

**File**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/pathmap_par_integration.rs`

**Lines 136-259**: Replace raw byte collection with `dump_all_sexpr()`

```rust
// Remove the workaround (lines 136-196)
let mut all_paths_data = Vec::new();
let mut rz = space.btm.read_zipper();
// ... manual serialization code ...

// Restore to:
let mut space_dump_bytes = Vec::new();
space.dump_all_sexpr(&mut space_dump_bytes)
    .map_err(|e| format!("Failed to dump space: {:?}", e))?;

let space_bytes_par = Par::default().with_exprs(vec![Expr {
    expr_instance: Some(ExprInstance::GByteArray(space_dump_bytes)),
}]);
```

**Lines 549-633**: Replace raw byte insertion with `load_all_sexpr()`

```rust
// Remove the workaround (lines 549-633)
// ... manual deserialization code ...

// Restore to:
if !space_dump_bytes.is_empty() {
    space.load_all_sexpr(&space_dump_bytes[..])
        .map_err(|e| format!("Failed to load space: {:?}", e))?;
}
```

**Update documentation**: Remove the comment at lines 130-136 that explains why we avoid `dump_all_sexpr()`

#### Step 2: Restore serialize_mork_expr()

**File**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/types.rs`

**Lines 150-170**: Promote `serialize_mork_expr_old()` to `serialize_mork_expr()`

```rust
// Remove deprecation markers and rename
pub(crate) fn serialize_mork_expr(expr: &mork_bytestring::Expr, space: &Space) -> String {
    let mut buffer = Vec::new();
    expr.serialize2(&mut buffer,
        |s| {
            #[cfg(feature="interning")]
            {
                let symbol = i64::from_be_bytes(s.try_into().unwrap()).to_be_bytes();
                let mstr = space.sm.get_bytes(symbol).map(|x| unsafe { std::str::from_utf8_unchecked(x) });
                unsafe { std::mem::transmute(mstr.unwrap_or("")) }
            }
            #[cfg(not(feature="interning"))]
            unsafe { std::mem::transmute(std::str::from_utf8_unchecked(s)) }
        },
        |i, _intro| { mork_bytestring::Expr::VARNAMES[i as usize] });

    String::from_utf8_lossy(&buffer).to_string()
}
```

**Replace calls to `mork_expr_to_metta_value()`**:

1. **`get_type()` (line 200)**:
```rust
// OLD (WORKAROUND):
if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
    // ... check if type assertion
}

// NEW (RESTORED):
let value_str = Self::serialize_mork_expr(&expr, &space);
if let Ok(value) = compile(&value_str) {
    if let Some(value) = value.source.first() {
        // ... check if type assertion
    }
}
```

2. **`iter_rules()` (line 244)**:
```rust
// OLD (WORKAROUND):
if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
    if let MettaValue::SExpr(items) = &value {
        // ... check for rule
    }
}

// NEW (RESTORED):
let value_str = Self::serialize_mork_expr(&expr, &space);
if let Ok(state) = compile(&value_str) {
    if let Some(value) = state.source.first() {
        if let MettaValue::SExpr(items) = value {
            // ... check for rule
        }
    }
}
```

3. **`match_space()` (line 291)**:
```rust
// OLD (WORKAROUND):
if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
    if let Some(bindings) = pattern_match(pattern, &atom) {
        // ... apply bindings
    }
}

// NEW (RESTORED):
let atom_str = Self::serialize_mork_expr(&expr, &space);
if let Ok(state) = compile(&atom_str) {
    if let Some(atom) = state.source.first() {
        if let Some(bindings) = pattern_match(pattern, atom) {
            // ... apply bindings
        }
    }
}
```

4. **`has_sexpr_fact()` (line 397)**:
```rust
// OLD (WORKAROUND):
if let Ok(stored_value) = Self::mork_expr_to_metta_value(&expr, &space) {
    if sexpr.structurally_equivalent(&stored_value) {
        return true;
    }
}

// NEW (RESTORED):
let stored_str = Self::serialize_mork_expr(&expr, &space);
if let Ok(state) = compile(&stored_str) {
    if let Some(stored_value) = state.source.first() {
        if sexpr.structurally_equivalent(stored_value) {
            return true;
        }
    }
}
```

**Keep `mork_expr_to_metta_value()` as fallback**:
```rust
// Rename to indicate it's a workaround/fallback
#[allow(dead_code)]
pub(crate) fn mork_expr_to_metta_value_fallback(expr: &mork_bytestring::Expr, space: &Space) -> Result<MettaValue, String> {
    // ... existing implementation
}
```

#### Step 3: Restore mork_bindings_to_metta()

**File**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/mork_convert.rs`

**Lines 195-200**: Use `serialize_mork_expr()` instead of `mork_expr_to_metta_value()`

```rust
// OLD (WORKAROUND):
let expr = expr_env.subsexpr();
if let Ok(value) = Environment::mork_expr_to_metta_value(&expr, space) {
    bindings.insert(format!("${}", var_name), value);
}

// NEW (RESTORED):
use super::compile::compile;
let expr = expr_env.subsexpr();
let value_str = Environment::serialize_mork_expr(&expr, space);
if let Ok(state) = compile(&value_str) {
    if let Some(value) = state.source.first() {
        bindings.insert(format!("${}", var_name), value.clone());
    }
}
```

#### Step 4: Update Documentation

**Files to update**:
1. `EVALUATION_SERIALIZATION_FIX.md` - Mark as resolved
2. `FIX_SUMMARY.md` - Mark as resolved
3. `CLAUDE.md` - Remove warnings about serialize2

**Add restoration note**:
```markdown
## serialize2() Restoration (YYYY-MM-DD)

MORK team fixed the reserved byte bug in version X.Y.Z.
Restored serialize2() usage throughout the codebase.

Changes:
- Reverted PathMap Par integration to use dump_all_sexpr()
- Restored serialize_mork_expr() for Expr to string conversion
- Updated mork_bindings_to_metta() to use serialize2-based conversion

All tests pass with the restored serialize2() implementation.
```

#### Step 5: Run Tests

**Verify everything works**:
```bash
cd /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler

# Run all tests
env RUSTFLAGS="-C target-cpu=native" cargo test --lib

# Run reserved byte regression tests specifically
env RUSTFLAGS="-C target-cpu=native" cargo test --lib pathmap_par_integration::tests::test_reserved

# Test with robot planning example
env RUSTFLAGS="-C target-cpu=native" cargo build --release
cd /home/dylon/Workspace/f1r3fly.io/f1r3node
./target/release/rholang-cli examples/robot_planning.rho
```

**Expected results**:
- All 15 pathmap_par_integration tests pass
- No panics with reserved bytes
- robot_planning.rho executes successfully

## Code Locations Reference

### Files Using serialize2() Workarounds

| File | Function | Lines | Workaround Type |
|------|----------|-------|-----------------|
| `src/pathmap_par_integration.rs` | `environment_to_par()` | 136-259 | Raw byte collection instead of `dump_all_sexpr()` |
| `src/pathmap_par_integration.rs` | `par_to_environment()` | 549-633 | Raw byte insertion instead of `load_all_sexpr()` |
| `src/backend/types.rs` | `mork_expr_to_metta_value()` | 76-216 | Direct parsing with `maybe_byte_item()` |
| `src/backend/types.rs` | `serialize_mork_expr_old()` | 150-170 | Deprecated (old serialize2 implementation) |
| `src/backend/types.rs` | `get_type()` | 200 | Uses `mork_expr_to_metta_value()` |
| `src/backend/types.rs` | `iter_rules()` | 244 | Uses `mork_expr_to_metta_value()` |
| `src/backend/types.rs` | `match_space()` | 291 | Uses `mork_expr_to_metta_value()` |
| `src/backend/types.rs` | `has_sexpr_fact()` | 397 | Uses `mork_expr_to_metta_value()` |
| `src/backend/mork_convert.rs` | `mork_bindings_to_metta()` | 195-200 | Uses `mork_expr_to_metta_value()` |

### Test Files for Verification

| Test | File | Line | Purpose |
|------|------|------|---------|
| `test_reserved_bytes_roundtrip_y_z` | `src/pathmap_par_integration.rs` | 985 | Tests 'y' (121) and 'z' (122) |
| `test_reserved_bytes_roundtrip_tilde` | `src/pathmap_par_integration.rs` | 1014 | Tests '~' (126) - the bug report case |
| `test_reserved_bytes_with_rules` | `src/pathmap_par_integration.rs` | 1089 | Tests original bug scenario |
| `test_reserved_bytes_multiple_roundtrips` | `src/pathmap_par_integration.rs` | 1045 | Tests stability across multiple serializations |
| `test_reserved_bytes_all_range` | `src/pathmap_par_integration.rs` | 1140 | Tests entire reserved range (64-127) |
| `test_reserved_bytes_robot_planning_regression` | `src/pathmap_par_integration.rs` | 1172 | Tests robot_planning.rho symbols |
| `test_reserved_bytes_with_evaluation` | `src/pathmap_par_integration.rs` | 1232 | Tests deserialized environments can evaluate |

## Performance Considerations

### Current Workaround Performance

**Advantages**:
- **Faster serialization**: Raw byte collection is faster than serialize2's text conversion
- **No string allocation**: Avoids intermediate string buffers
- **Direct MORK access**: Bypasses text serialization layer

**Disadvantages**:
- **More complex code**: Manual byte collection and parsing
- **Larger code surface**: More places for bugs to hide
- **Harder to maintain**: Changes to MORK format require updates in multiple places

### serialize2() Performance (Once Restored)

**Advantages**:
- **Simpler code**: Single function call instead of manual parsing
- **MORK-maintained**: Format changes handled by MORK team
- **Cleaner API**: Well-defined interface

**Disadvantages**:
- **Text conversion overhead**: Converts bytes → text → bytes
- **String allocation**: Creates intermediate string buffers
- **Potentially slower**: May be 10-30% slower than raw byte collection

**Recommendation**: Keep raw byte collection approach for PathMap Par integration even after serialize2() is fixed, but restore serialize2() for `types.rs` and `mork_convert.rs` where text conversion is necessary.

## Related Documentation

- **Evaluation Fix**: `EVALUATION_SERIALIZATION_FIX.md`
- **PathMap Fix**: `FIX_SUMMARY.md`
- **Multiplicities Optimization**: `MULTIPLICITIES_BYTE_ARRAY_OPTIMIZATION.md`
- **Pretty Printer**: `PRETTY_PRINTER_METTA_DISPLAY.md`
- **MORK Bug Reports**: `/home/dylon/Workspace/f1r3fly.io/MORK/benchmarks/reserved-bug-test/`

## Change Log

### 2025-10-17: Initial Documentation
- Created restoration guide
- Documented all workarounds
- Provided step-by-step restoration instructions
- Listed all affected code locations

---

**NOTE**: This document should be updated when MORK fixes the serialize2() bug and when restoration is performed.
