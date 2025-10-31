# Multiplicities Byte Array Optimization

## Summary

Changed `multiplicities` serialization in `Environment` from EMap (Rholang map) to GByteArray (binary format) for efficiency and consistency with the `space` field.

## Status: ✅ COMPLETE

All tests pass (15/15 in pathmap_par_integration).

## Motivation

The `space` field was changed to use GByteArray to avoid the reserved byte bug and improve efficiency. The `multiplicities` field was still using EMap, which was:
- Inconsistent with `space` field approach
- Less efficient for serialization/deserialization
- Larger serialized size due to Par wrapper overhead
- Not necessary for Rholang debugging (multiplicities are internal state)

## Changes Made

### File: `src/pathmap_par_integration.rs`

#### 1. Serialization (`environment_to_par`)

**Before** (lines 196-215):
```rust
// Serialize multiplicities as an EMap (Rholang map)
let multiplicities_map = env.get_multiplicities();
let mut multiplicities_kvs = Vec::new();
for (rule_key, count) in multiplicities_map.iter() {
    let key_par = create_string_par(rule_key.clone());
    let value_par = create_int_par(*count as i64);
    multiplicities_kvs.push(KeyValuePair {
        key: Some(key_par),
        value: Some(value_par),
    });
}

let multiplicities_emap = Par::default().with_exprs(vec![Expr {
    expr_instance: Some(ExprInstance::EMapBody(EMap {
        kvs: multiplicities_kvs,
        locally_free: Vec::new(),
        connective_used: false,
        remainder: None,
    })),
}]);
```

**After** (lines 196-221):
```rust
// Serialize multiplicities as a byte array for efficiency and consistency
// Format: [count: 8 bytes][key1_len: 4 bytes][key1_bytes][value1: 8 bytes]...
let multiplicities_map = env.get_multiplicities();
let mut multiplicities_bytes = Vec::new();

// Write count
let count = multiplicities_map.len() as u64;
multiplicities_bytes.extend_from_slice(&count.to_be_bytes());

// Write each key-value pair
for (rule_key, count) in multiplicities_map.iter() {
    let key_bytes = rule_key.as_bytes();
    // Write key length (4 bytes)
    let key_len = key_bytes.len() as u32;
    multiplicities_bytes.extend_from_slice(&key_len.to_be_bytes());
    // Write key bytes
    multiplicities_bytes.extend_from_slice(key_bytes);
    // Write value (8 bytes)
    multiplicities_bytes.extend_from_slice(&(*count as u64).to_be_bytes());
}

let multiplicities_emap = Par::default().with_exprs(vec![Expr {
    expr_instance: Some(ExprInstance::GByteArray(multiplicities_bytes)),
}]);
```

#### 2. Deserialization (`par_to_environment`)

**Before** (lines 464-487):
```rust
// Extract multiplicities (element 1)
let multiplicities_par = extract_tuple_value(&tuple.ps[1])?;
let mut multiplicities_map: HashMap<String, usize> = HashMap::new();
if let Some(expr) = multiplicities_par.exprs.first() {
    if let Some(ExprInstance::EMapBody(emap)) = &expr.expr_instance {
        for kv in &emap.kvs {
            // Extract key (Option<Par> containing string)
            if let Some(key_par) = &kv.key {
                if let Some(key_expr) = key_par.exprs.first() {
                    if let Some(ExprInstance::GString(key_str)) = &key_expr.expr_instance {
                        // Extract value (Option<Par> containing integer)
                        if let Some(value_par) = &kv.value {
                            if let Some(value_expr) = value_par.exprs.first() {
                                if let Some(ExprInstance::GInt(count)) = &value_expr.expr_instance {
                                    multiplicities_map.insert(key_str.clone(), *count as usize);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
```

**After** (lines 468-521):
```rust
// Extract multiplicities (element 1) - now stored as GByteArray
let multiplicities_par = extract_tuple_value(&tuple.ps[1])?;
let mut multiplicities_map: HashMap<String, usize> = HashMap::new();
if let Some(expr) = multiplicities_par.exprs.first() {
    if let Some(ExprInstance::GByteArray(mult_bytes)) = &expr.expr_instance {
        // Read format: [count: 8 bytes][key1_len: 4 bytes][key1_bytes][value1: 8 bytes]...
        if mult_bytes.len() >= 8 {
            let mut offset = 0;

            // Read count
            let count = u64::from_be_bytes([
                mult_bytes[offset], mult_bytes[offset+1],
                mult_bytes[offset+2], mult_bytes[offset+3],
                mult_bytes[offset+4], mult_bytes[offset+5],
                mult_bytes[offset+6], mult_bytes[offset+7],
            ]);
            offset += 8;

            // Read each key-value pair
            for _ in 0..count {
                if offset + 4 > mult_bytes.len() {
                    break; // Not enough data
                }

                // Read key length
                let key_len = u32::from_be_bytes([
                    mult_bytes[offset], mult_bytes[offset+1],
                    mult_bytes[offset+2], mult_bytes[offset+3],
                ]) as usize;
                offset += 4;

                if offset + key_len + 8 > mult_bytes.len() {
                    break; // Not enough data
                }

                // Read key bytes
                let key_bytes = &mult_bytes[offset..offset+key_len];
                let key = String::from_utf8_lossy(key_bytes).to_string();
                offset += key_len;

                // Read value
                let value = u64::from_be_bytes([
                    mult_bytes[offset], mult_bytes[offset+1],
                    mult_bytes[offset+2], mult_bytes[offset+3],
                    mult_bytes[offset+4], mult_bytes[offset+5],
                    mult_bytes[offset+6], mult_bytes[offset+7],
                ]) as usize;
                offset += 8;

                multiplicities_map.insert(key, value);
            }
        }
    }
}
```

#### 3. Removed Unused Imports

Removed `EMap` and `KeyValuePair` from imports (line 7):
```rust
use models::rhoapi::{Par, Expr, expr::ExprInstance, EPathMap, EList, ETuple};
```

#### 4. Updated Documentation

Updated function documentation to reflect byte array format (lines 117-122, 432-436).

#### 5. Updated Tests

Updated test to check for GByteArray instead of EMapBody (lines 756-769).

## Binary Format

### Multiplicities Format:
```
[count: 8 bytes (u64, big-endian)]
For each entry:
  [key_len: 4 bytes (u32, big-endian)]
  [key_bytes: UTF-8 string]
  [value: 8 bytes (u64, big-endian)]
```

### Example:
```
Count = 2
Entry 1: key="double_$x_*_$x_2" (len=17), value=1
Entry 2: key="foo_$a" (len=6), value=3

Bytes:
00 00 00 00 00 00 00 02  // count = 2
00 00 00 11              // key1_len = 17
64 6F 75 62 6C 65 ...   // "double_$x_*_$x_2"
00 00 00 00 00 00 00 01  // value1 = 1
00 00 00 06              // key2_len = 6
66 6F 6F 5F 24 61        // "foo_$a"
00 00 00 00 00 00 00 03  // value2 = 3
```

## Benefits

### 1. **Consistency**
- Both `space` and `multiplicities` use GByteArray
- Uniform serialization approach across the Environment

### 2. **Efficiency**
- **Smaller size**: No Par wrapper overhead for each key-value pair
- **Faster serialization**: Direct byte writes instead of constructing nested Par structures
- **Faster deserialization**: Direct byte reads instead of traversing nested structures

### 3. **Simplicity**
- Binary format is straightforward and easy to understand
- No need to handle complex nested Par structures
- Easier to maintain and debug

### 4. **Future-proof**
- Scales better for large maps (1000+ entries)
- No dependency on Rholang's EMap structure
- Can easily extend format if needed

## Size Comparison

### For a map with 10 entries (average key length 20 bytes):

**EMap approach:**
- Each key: ~50 bytes (Par + Expr + GString + overhead)
- Each value: ~30 bytes (Par + Expr + GInt + overhead)
- Total: ~800 bytes

**Byte array approach:**
- Each entry: 4 + 20 + 8 = 32 bytes
- Count: 8 bytes
- Total: ~328 bytes

**Savings: ~59% reduction in size**

## Testing

All 15 pathmap_par_integration tests pass:
- ✅ Basic serialization/deserialization
- ✅ Round-trip with reserved bytes
- ✅ Multiple round-trips
- ✅ Evaluation after deserialization
- ✅ Robot planning regression test

```bash
env RUSTFLAGS="-C target-cpu=native" cargo test --lib pathmap_par_integration::tests
```

Result: **15 passed; 0 failed**

## Backward Compatibility

⚠️ **Breaking Change**: This changes the wire format for Environment serialization.

Any existing serialized MettaState objects with the old EMap format will not deserialize correctly.

**Migration Path:**
1. Existing Rholang contracts using the old format need to be updated
2. No data migration needed if starting fresh
3. This is an internal format change - the Rust API remains unchanged

## Related Documentation

- **Reserved Byte Fix**: `EVALUATION_SERIALIZATION_FIX.md`
- **PathMap Integration**: `FIX_SUMMARY.md`

## Performance Notes

This optimization is particularly beneficial when:
- Environments have many rules (100+ multiplicities entries)
- Frequent serialization/deserialization (RPC calls)
- Memory-constrained environments
- Network bandwidth is limited

For small environments (< 10 rules), the difference is negligible but the consistency is valuable.
