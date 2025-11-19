# Rholang Integration Guide for MORK Spaces

**Version**: 1.0
**Date**: 2025-11-13
**Target**: MeTTaTron Compiler
**Hardware Reference**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads), 252 GB DDR4-2133 ECC

---

## Table of Contents

1. [Introduction](#introduction)
2. [Rholang Par Type Overview](#rholang-par-type-overview)
3. [Current Integration Architecture](#current-integration-architecture)
4. [Binary Format Specification](#binary-format-specification)
5. [Serialization Implementation](#serialization-implementation)
6. [Deserialization Implementation](#deserialization-implementation)
7. [Optimization Strategies](#optimization-strategies)
8. [Error Handling](#error-handling)
9. [Versioning and Compatibility](#versioning-and-compatibility)
10. [Best Practices](#best-practices)

---

## Introduction

This guide documents the integration between MeTTaTron's MORK Spaces and the Rholang runtime, focusing on efficient serialization and deserialization strategies.

### Integration Goals

1. **Bidirectional Communication**: Send MORK Spaces to Rholang and receive them back
2. **Efficiency**: Minimize serialization overhead
3. **Integrity**: Ensure data consistency across the boundary
4. **Compatibility**: Support Rholang's Protobuf-based data model

### Key Challenge

**MeTTa (MORK)** ↔ **Rholang (Protobuf)**

- MORK uses custom binary encoding (tag-based, De Bruijn levels)
- Rholang uses Protobuf Par types
- Need efficient, lossless mapping

---

## Rholang Par Type Overview

### Protobuf Definition

**Location**: `/home/dylon/Workspace/f1r3fly.io/f1r3node/models/src/main/protobuf/RhoTypes.proto`

```protobuf
message Par {
  repeated Send sends = 1;
  repeated Receive receives = 2;
  repeated New news = 4;
  repeated Expr exprs = 5;
  repeated Match matches = 6;
  repeated GUnforgeable unforgeables = 7;
  repeated Bundle bundles = 11;
  repeated Connective connectives = 8;
  bytes locallyFree = 9;
  bool connective_used = 10;
}

message Expr {
  oneof expr_instance {
    GBool g_bool = 1;
    GInt g_int = 2;
    GString g_string = 3;
    GUri g_uri = 4;
    GByteArray g_byte_array = 5;
    EList e_list = 7;
    ETuple e_tuple = 8;
    EPathMap e_path_map = 100;
    // ... other types
  }
}
```

### Relevant Types for MORK Serialization

**GByteArray**: Opaque binary data
```protobuf
message GByteArray {
  bytes value = 1;
}
```

**ETuple**: Structured tuple
```protobuf
message ETuple {
  repeated Par ps = 1;
}
```

**EPathMap**: Specialized for PathMap (experimental)
```protobuf
message EPathMap {
  map<string, Par> data = 1;
}
```

---

## Current Integration Architecture

### File Structure

**Location**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/pathmap_par_integration.rs`

### Current Approach

```rust
pub fn environment_to_par(env: &Environment) -> Par {
    // 1. Create temporary MORK space from environment
    let space = env.create_space();
    let multiplicities = env.get_multiplicities();

    // 2. Serialize to binary
    let space_bytes = serialize_space_to_bytes(&space);
    let mult_bytes = serialize_multiplicities(&multiplicities);

    // 3. Package in Par as ETuple with labeled data
    Par::default().with_exprs(vec![
        Expr {
            expr_instance: Some(ExprInstance::ETuple(ETuple {
                ps: vec![
                    // Label: "space"
                    Par::default().with_exprs(vec![Expr {
                        expr_instance: Some(ExprInstance::GString(GString {
                            value: "space".to_string(),
                        })),
                    }]),
                    // Data: GByteArray
                    Par::default().with_exprs(vec![Expr {
                        expr_instance: Some(ExprInstance::GByteArray(GByteArray {
                            value: space_bytes,
                        })),
                    }]),

                    // Label: "multiplicities"
                    Par::default().with_exprs(vec![Expr {
                        expr_instance: Some(ExprInstance::GString(GString {
                            value: "multiplicities".to_string(),
                        })),
                    }]),
                    // Data: GByteArray
                    Par::default().with_exprs(vec![Expr {
                        expr_instance: Some(ExprInstance::GByteArray(GByteArray {
                            value: mult_bytes,
                        })),
                    }]),
                ],
            })),
        }],
    ])
}
```

### Data Flow

```
MeTTa Environment
    ↓
MORK Space + Multiplicities
    ↓
Binary Serialization
    ↓
Rholang Par (ETuple)
    ↓
Network/Storage
    ↓
Rholang Runtime
```

---

## Binary Format Specification

### Space Serialization Format

```
┌────────────────────────────────────────────────────────────┐
│ MORK Space Binary Format                                   │
├────────────────────────────────────────────────────────────┤
│ [4 bytes: Magic "MTTS"]                                    │
│ [2 bytes: Version (currently 1)]                           │
│ [2 bytes: Flags (reserved)]                                │
│ [8 bytes: Symbol table length]                             │
│ [N bytes: Symbol table (ZIP compressed)]                   │
│ [8 bytes: Number of paths]                                 │
│ [For each path:                                            │
│   [4 bytes: Path length]                                   │
│   [M bytes: Path data]                                     │
│ ]                                                          │
│ [32 bytes: Blake2b checksum (optional)]                    │
└────────────────────────────────────────────────────────────┘
```

### Multiplicities Format

```
┌────────────────────────────────────────────────────────────┐
│ Multiplicities Binary Format                               │
├────────────────────────────────────────────────────────────┤
│ [4 bytes: Magic "MTTM"]                                    │
│ [2 bytes: Version]                                         │
│ [2 bytes: Flags]                                           │
│ [8 bytes: Number of entries]                               │
│ [For each entry:                                           │
│   [4 bytes: Key length]                                    │
│   [N bytes: Key (UTF-8 string)]                            │
│   [8 bytes: Value (usize as u64)]                          │
│ ]                                                          │
└────────────────────────────────────────────────────────────┘
```

---

## Serialization Implementation

### Complete Space Serialization

```rust
pub fn serialize_space_to_bytes(space: &Space) -> Vec<u8> {
    let mut buffer = Vec::new();

    // 1. Write magic and header
    buffer.extend_from_slice(b"MTTS");  // Magic
    buffer.extend_from_slice(&1u16.to_le_bytes());  // Version
    buffer.extend_from_slice(&0u16.to_le_bytes());  // Flags

    // 2. Serialize symbol table to temp file
    let temp_dir = std::env::temp_dir();
    let sym_path = temp_dir.join(format!("sym_{}.zip", process::id()));

    space.sm.serialize(&sym_path).expect("Failed to serialize symbol table");

    // 3. Read symbol table back as bytes
    let sym_bytes = std::fs::read(&sym_path).expect("Failed to read symbol table");
    std::fs::remove_file(&sym_path).ok();  // Clean up

    // 4. Write symbol table
    buffer.extend_from_slice(&(sym_bytes.len() as u64).to_le_bytes());
    buffer.extend_from_slice(&sym_bytes);

    // 5. Collect all paths
    let zipper = space.btm.read_zipper();
    let paths: Vec<Vec<u8>> = zipper.iter_paths()
        .map(|path| path.to_vec())
        .collect();

    // 6. Write path count
    buffer.extend_from_slice(&(paths.len() as u64).to_le_bytes());

    // 7. Write each path
    for path in paths {
        buffer.extend_from_slice(&(path.len() as u32).to_le_bytes());
        buffer.extend_from_slice(&path);
    }

    // 8. Optional: Add checksum
    if ENABLE_CHECKSUMS {
        let checksum = compute_blake2b(&buffer);
        buffer.extend_from_slice(&checksum);
    }

    buffer
}
```

### Multiplicities Serialization

```rust
pub fn serialize_multiplicities(multiplicities: &HashMap<String, usize>) -> Vec<u8> {
    let mut buffer = Vec::new();

    // 1. Write header
    buffer.extend_from_slice(b"MTTM");  // Magic
    buffer.extend_from_slice(&1u16.to_le_bytes());  // Version
    buffer.extend_from_slice(&0u16.to_le_bytes());  // Flags

    // 2. Write count
    buffer.extend_from_slice(&(multiplicities.len() as u64).to_le_bytes());

    // 3. Write entries (sorted for determinism)
    let mut entries: Vec<_> = multiplicities.iter().collect();
    entries.sort_by_key(|(k, _)| *k);

    for (key, value) in entries {
        let key_bytes = key.as_bytes();

        // Write key
        buffer.extend_from_slice(&(key_bytes.len() as u32).to_le_bytes());
        buffer.extend_from_slice(key_bytes);

        // Write value
        buffer.extend_from_slice(&(*value as u64).to_le_bytes());
    }

    buffer
}
```

### Optimized Version (Avoid Temp File)

```rust
pub fn serialize_space_to_bytes_v2(space: &Space) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(estimate_serialized_size(space));

    // 1. Write header
    write_header(&mut buffer, b"MTTS", 1, 0);

    // 2. Serialize symbol table directly to buffer
    let sym_offset = buffer.len();
    buffer.extend_from_slice(&0u64.to_le_bytes());  // Placeholder for length

    let sym_start = buffer.len();
    serialize_symbol_table_inline(&space.sm, &mut buffer)?;
    let sym_len = buffer.len() - sym_start;

    // Update length
    let sym_len_bytes = (sym_len as u64).to_le_bytes();
    buffer[sym_offset..sym_offset + 8].copy_from_slice(&sym_len_bytes);

    // 3. Serialize paths
    serialize_paths_inline(&space.btm, &mut buffer)?;

    // 4. Add checksum
    if ENABLE_CHECKSUMS {
        let checksum = compute_blake2b(&buffer);
        buffer.extend_from_slice(&checksum);
    }

    buffer
}

fn serialize_symbol_table_inline(
    sm: &SharedMappingHandle,
    buffer: &mut Vec<u8>,
) -> io::Result<()> {
    // Create ZIP in memory
    let mut zip_buffer = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut zip_buffer);

    // Write symbol table entries to ZIP
    write_str_to_sym_inline(&mut zip, &sm.str_to_sym.read())?;
    write_sym_to_str_inline(&mut zip, &sm.sym_to_str.read())?;
    // ... etc

    zip.finish()?;

    // Append ZIP bytes to buffer
    buffer.extend_from_slice(&zip_buffer.into_inner());

    Ok(())
}

fn serialize_paths_inline(btm: &PathMap<()>, buffer: &mut Vec<u8>) -> io::Result<()> {
    let zipper = btm.read_zipper();

    // Count paths
    let path_count = btm.val_count();
    buffer.extend_from_slice(&path_count.to_le_bytes());

    // Write paths
    for path in zipper.iter_paths() {
        buffer.extend_from_slice(&(path.len() as u32).to_le_bytes());
        buffer.extend_from_slice(path);
    }

    Ok(())
}
```

---

## Deserialization Implementation

### Par to Space Conversion

```rust
pub fn par_to_environment(par: &Par) -> Result<Environment, DeserializationError> {
    // 1. Extract ETuple
    let tuple = extract_tuple_from_par(par)?;

    // 2. Parse tuple: [("space", space_bytes), ("multiplicities", mult_bytes)]
    let (space_bytes, mult_bytes) = parse_labeled_tuple(&tuple)?;

    // 3. Deserialize space
    let space = deserialize_space_from_bytes(&space_bytes)?;

    // 4. Deserialize multiplicities
    let multiplicities = deserialize_multiplicities(&mult_bytes)?;

    // 5. Reconstruct environment
    Ok(Environment::from_space_and_multiplicities(space, multiplicities))
}

fn deserialize_space_from_bytes(bytes: &[u8]) -> Result<Space, DeserializationError> {
    let mut pos = 0;

    // 1. Read and validate header
    let magic = read_bytes(bytes, &mut pos, 4)?;
    if magic != b"MTTS" {
        return Err(DeserializationError::InvalidMagic);
    }

    let version = read_u16(bytes, &mut pos)?;
    if version != 1 {
        return Err(DeserializationError::UnsupportedVersion(version));
    }

    let _flags = read_u16(bytes, &mut pos)?;

    // 2. Read symbol table
    let sym_len = read_u64(bytes, &mut pos)? as usize;
    let sym_bytes = read_bytes(bytes, &mut pos, sym_len)?;

    // Write to temp file and deserialize
    let temp_path = write_temp_file(sym_bytes)?;
    let sm = SharedMapping::deserialize(&temp_path)?;
    std::fs::remove_file(&temp_path).ok();

    // 3. Read paths
    let path_count = read_u64(bytes, &mut pos)? as usize;

    let mut btm = PathMap::new();
    let mut wz = btm.write_zipper();

    for _ in 0..path_count {
        let path_len = read_u32(bytes, &mut pos)? as usize;
        let path = read_bytes(bytes, &mut pos, path_len)?;

        let source = BTMSource::new(path.to_vec());
        wz.join_into(&source.read_zipper(), true);
    }

    // 4. Verify checksum if present
    if ENABLE_CHECKSUMS && pos + 32 <= bytes.len() {
        let expected_checksum = &bytes[pos..pos + 32];
        let actual_checksum = compute_blake2b(&bytes[..pos]);

        if expected_checksum != actual_checksum {
            return Err(DeserializationError::ChecksumMismatch);
        }
    }

    Ok(Space {
        btm,
        sm,
        mmaps: HashMap::new(),
    })
}

fn deserialize_multiplicities(bytes: &[u8]) -> Result<HashMap<String, usize>, DeserializationError> {
    let mut pos = 0;

    // 1. Validate header
    let magic = read_bytes(bytes, &mut pos, 4)?;
    if magic != b"MTTM" {
        return Err(DeserializationError::InvalidMagic);
    }

    let version = read_u16(bytes, &mut pos)?;
    if version != 1 {
        return Err(DeserializationError::UnsupportedVersion(version));
    }

    let _flags = read_u16(bytes, &mut pos)?;

    // 2. Read count
    let count = read_u64(bytes, &mut pos)? as usize;

    // 3. Read entries
    let mut multiplicities = HashMap::with_capacity(count);

    for _ in 0..count {
        let key_len = read_u32(bytes, &mut pos)? as usize;
        let key_bytes = read_bytes(bytes, &mut pos, key_len)?;
        let key = String::from_utf8(key_bytes.to_vec())
            .map_err(|_| DeserializationError::InvalidUtf8)?;

        let value = read_u64(bytes, &mut pos)? as usize;

        multiplicities.insert(key, value);
    }

    Ok(multiplicities)
}

// Helper functions
fn read_bytes<'a>(bytes: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8], DeserializationError> {
    if *pos + len > bytes.len() {
        return Err(DeserializationError::UnexpectedEof);
    }

    let result = &bytes[*pos..*pos + len];
    *pos += len;
    Ok(result)
}

fn read_u16(bytes: &[u8], pos: &mut usize) -> Result<u16, DeserializationError> {
    let slice = read_bytes(bytes, pos, 2)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, DeserializationError> {
    let slice = read_bytes(bytes, pos, 4)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], pos: &mut usize) -> Result<u64, DeserializationError> {
    let slice = read_bytes(bytes, pos, 8)?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3],
        slice[4], slice[5], slice[6], slice[7],
    ]))
}
```

---

## Optimization Strategies

### 1. Avoid Temporary Files

**Problem**: Current implementation writes symbol table to temp file

**Solution**: In-memory ZIP creation

```rust
use std::io::Cursor;

fn serialize_symbol_table_to_bytes(sm: &SharedMappingHandle) -> io::Result<Vec<u8>> {
    let mut buffer = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buffer);

    // Write all maps
    write_maps_to_zip(&mut zip, sm)?;

    zip.finish()?;
    Ok(buffer.into_inner())
}
```

### 2. Batch Path Collection

**Problem**: Iterating paths one by one is slow

**Solution**: Collect in batches, use parallel processing

```rust
fn collect_paths_parallel(btm: &PathMap<()>) -> Vec<Vec<u8>> {
    use rayon::prelude::*;

    let zipper = btm.read_zipper();

    // Collect paths in parallel
    let paths: Vec<Vec<u8>> = zipper.iter_paths()
        .par_bridge()  // Parallel iterator
        .map(|path| path.to_vec())
        .collect();

    paths
}
```

### 3. Preallocate Buffer

**Problem**: Vec reallocations during serialization

**Solution**: Estimate size and preallocate

```rust
fn estimate_serialized_size(space: &Space) -> usize {
    let header_size = 16;  // Magic + version + flags
    let sym_table_size = estimate_symbol_table_size(&space.sm);
    let path_count_size = 8;
    let avg_path_size = 50;  // Estimate
    let path_data_size = space.btm.val_count() * (4 + avg_path_size);
    let checksum_size = 32;

    header_size + 8 + sym_table_size + path_count_size + path_data_size + checksum_size
}

fn estimate_symbol_table_size(sm: &SharedMappingHandle) -> usize {
    // Rough estimate: 20 bytes overhead + avg 10 bytes per symbol
    let symbol_count = sm.str_to_sym.read().len();
    (symbol_count * 30) / 3  // Assume 3:1 compression
}
```

### 4. Streaming Serialization

**Problem**: Large spaces don't fit in memory

**Solution**: Stream directly to output

```rust
pub fn serialize_space_streaming<W: Write>(
    space: &Space,
    mut writer: W,
) -> io::Result<()> {
    // 1. Write header
    writer.write_all(b"MTTS")?;
    writer.write_all(&1u16.to_le_bytes())?;
    writer.write_all(&0u16.to_le_bytes())?;

    // 2. Stream symbol table
    stream_symbol_table(&space.sm, &mut writer)?;

    // 3. Stream paths
    let path_count = space.btm.val_count();
    writer.write_all(&path_count.to_le_bytes())?;

    let zipper = space.btm.read_zipper();
    for path in zipper.iter_paths() {
        writer.write_all(&(path.len() as u32).to_le_bytes())?;
        writer.write_all(path)?;
    }

    Ok(())
}
```

---

## Error Handling

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum DeserializationError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Invalid magic number")]
    InvalidMagic,

    #[error("Unsupported version: {0}")]
    UnsupportedVersion(u16),

    #[error("Checksum mismatch")]
    ChecksumMismatch,

    #[error("Unexpected end of file")]
    UnexpectedEof,

    #[error("Invalid UTF-8")]
    InvalidUtf8,

    #[error("Par extraction failed: {0}")]
    ParExtractionFailed(String),

    #[error("Corrupted data: {0}")]
    CorruptedData(String),
}
```

### Graceful Degradation

```rust
pub fn deserialize_with_recovery(bytes: &[u8]) -> Result<Space, DeserializationError> {
    match deserialize_space_from_bytes(bytes) {
        Ok(space) => Ok(space),
        Err(e) => {
            eprintln!("Deserialization failed: {}, attempting recovery", e);

            // Try to recover partial data
            match recover_partial_space(bytes) {
                Ok(space) => {
                    eprintln!("Partial recovery successful");
                    Ok(space)
                }
                Err(recovery_err) => {
                    eprintln!("Recovery failed: {}", recovery_err);
                    Err(e)
                }
            }
        }
    }
}

fn recover_partial_space(bytes: &[u8]) -> Result<Space, DeserializationError> {
    // Attempt to read as much as possible
    // Skip corrupted paths, reconstruct symbol table, etc.
    todo!("Implement recovery logic")
}
```

---

## Versioning and Compatibility

### Version Strategy

```rust
const SERIALIZATION_VERSION_V1: u16 = 1;  // Current
const SERIALIZATION_VERSION_V2: u16 = 2;  // Future

pub fn serialize_versioned(space: &Space, version: u16) -> Result<Vec<u8>, SerializationError> {
    match version {
        SERIALIZATION_VERSION_V1 => serialize_space_to_bytes(space),
        SERIALIZATION_VERSION_V2 => serialize_space_to_bytes_v2(space),
        _ => Err(SerializationError::UnsupportedVersion(version)),
    }
}

pub fn deserialize_versioned(bytes: &[u8]) -> Result<Space, DeserializationError> {
    // Read version from header
    if bytes.len() < 6 {
        return Err(DeserializationError::UnexpectedEof);
    }

    let version = u16::from_le_bytes([bytes[4], bytes[5]]);

    match version {
        SERIALIZATION_VERSION_V1 => deserialize_space_from_bytes(bytes),
        SERIALIZATION_VERSION_V2 => deserialize_space_from_bytes_v2(bytes),
        _ => Err(DeserializationError::UnsupportedVersion(version)),
    }
}
```

### Migration

```rust
pub fn migrate_v1_to_v2(v1_bytes: &[u8]) -> Result<Vec<u8>, MigrationError> {
    // 1. Deserialize as v1
    let space = deserialize_space_from_bytes(v1_bytes)?;

    // 2. Serialize as v2
    let v2_bytes = serialize_space_to_bytes_v2(&space)?;

    Ok(v2_bytes)
}
```

---

## Best Practices

### 1. Always Validate

```rust
fn validate_before_deserialize(bytes: &[u8]) -> Result<(), ValidationError> {
    // Check minimum size
    if bytes.len() < 64 {
        return Err(ValidationError::TooSmall);
    }

    // Validate magic
    if &bytes[0..4] != b"MTTS" {
        return Err(ValidationError::InvalidMagic);
    }

    // Check version
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version > CURRENT_VERSION {
        return Err(ValidationError::UnsupportedVersion(version));
    }

    Ok(())
}
```

### 2. Use Checksums

```rust
fn add_checksum(buffer: &mut Vec<u8>) {
    let checksum = compute_blake2b(buffer);
    buffer.extend_from_slice(&checksum);
}

fn verify_checksum(bytes: &[u8]) -> Result<(), ChecksumError> {
    if bytes.len() < 32 {
        return Err(ChecksumError::TooShort);
    }

    let data_len = bytes.len() - 32;
    let expected = &bytes[data_len..];
    let actual = compute_blake2b(&bytes[..data_len]);

    if expected != actual {
        return Err(ChecksumError::Mismatch);
    }

    Ok(())
}
```

### 3. Log Serialization Stats

```rust
pub fn serialize_with_logging(space: &Space) -> Vec<u8> {
    let start = Instant::now();
    let bytes = serialize_space_to_bytes(space);
    let duration = start.elapsed();

    info!("Serialized space: {} paths, {} bytes, {:?}",
        space.btm.val_count(),
        bytes.len(),
        duration
    );

    bytes
}
```

### 4. Test Round-Trips

```rust
#[test]
fn test_rholang_roundtrip() {
    let env = create_test_environment();

    // Serialize to Par
    let par = environment_to_par(&env);

    // Deserialize from Par
    let restored_env = par_to_environment(&par).unwrap();

    // Verify equality
    assert_environments_equal(&env, &restored_env);
}
```

---

## Summary

This guide has covered:

1. **Rholang Par Type**: Understanding Protobuf-based data model
2. **Current Integration**: ETuple with GByteArray approach
3. **Binary Format**: Detailed specification for space and multiplicities
4. **Serialization**: Complete implementation with optimizations
5. **Deserialization**: Robust parsing with error handling
6. **Optimization**: Avoiding temp files, parallelization, preallocation
7. **Error Handling**: Comprehensive error types and recovery
8. **Versioning**: Forward and backward compatibility
9. **Best Practices**: Validation, checksums, logging, testing

### Key Takeaways

- **Use GByteArray** for opaque MORK serialization (current best practice)
- **Avoid temp files** by using in-memory ZIP creation
- **Preallocate buffers** for better performance
- **Always validate** headers and checksums
- **Version headers** for future compatibility
- **Test round-trips** to ensure correctness

---

**Document Version**: 1.0
**Last Updated**: 2025-11-13
**Next Review**: After implementing optimized serialization
