# MORK Serialization Format Specifications

**Version**: 1.0
**Date**: 2025-11-13
**Target**: MeTTaTron Compiler

---

## Table of Contents

1. [Binary Format Specification](#binary-format-specification)
2. [Paths Format Specification](#paths-format-specification)
3. [ACT Format Specification](#act-format-specification)
4. [Symbol Table Format](#symbol-table-format)
5. [Multiplicities Format](#multiplicities-format)
6. [Versioning Strategy](#versioning-strategy)
7. [Schema Evolution](#schema-evolution)

---

## Binary Format Specification

### Complete Format Layout

```
┌───────────────────────────────────────────────────────────────┐
│ MORK Space Binary Format v1                                   │
├───────────────────────────────────────────────────────────────┤
│ HEADER (16 bytes)                                             │
│   [0-3]:   Magic number "MTTS" (0x4D 0x54 0x54 0x53)        │
│   [4-5]:   Version (u16 LE) = 1                              │
│   [6-7]:   Flags (u16 LE)                                    │
│      Bit 0: Checksum enabled                                 │
│      Bit 1: Compression enabled                              │
│      Bit 2-15: Reserved                                      │
│   [8-15]:  Reserved (8 bytes, zeros)                         │
├───────────────────────────────────────────────────────────────┤
│ SYMBOL TABLE                                                  │
│   [0-7]:   Symbol table length (u64 LE)                      │
│   [8-N]:   Symbol table data (ZIP compressed)                │
├───────────────────────────────────────────────────────────────┤
│ PATHS DATA                                                    │
│   [0-7]:   Number of paths (u64 LE)                          │
│   For each path:                                             │
│     [0-3]:   Path length (u32 LE)                            │
│     [4-N]:   Path bytes (BTM encoded)                        │
├───────────────────────────────────────────────────────────────┤
│ FOOTER (optional, if checksum enabled)                       │
│   [0-31]:  Blake2b-256 checksum                              │
└───────────────────────────────────────────────────────────────┘
```

### Flag Bits

```rust
pub struct FormatFlags {
    pub checksum_enabled: bool,      // Bit 0
    pub compression_enabled: bool,   // Bit 1
    // Bits 2-15 reserved
}

impl FormatFlags {
    pub fn to_u16(&self) -> u16 {
        let mut flags = 0u16;
        if self.checksum_enabled {
            flags |= 1 << 0;
        }
        if self.compression_enabled {
            flags |= 1 << 1;
        }
        flags
    }

    pub fn from_u16(value: u16) -> Self {
        Self {
            checksum_enabled: (value & (1 << 0)) != 0,
            compression_enabled: (value & (1 << 1)) != 0,
        }
    }
}
```

### Example Binary Layout

For a space with 3 atoms:
```
4D 54 54 53                          # Magic "MTTS"
01 00                                 # Version 1
01 00                                 # Flags: checksum enabled
00 00 00 00 00 00 00 00              # Reserved

2A 00 00 00 00 00 00 00              # Symbol table length: 42 bytes
[42 bytes of ZIP compressed data]    # Symbol table

03 00 00 00 00 00 00 00              # 3 paths

0A 00 00 00                          # Path 1: 10 bytes
[10 bytes of path data]

0C 00 00 00                          # Path 2: 12 bytes
[12 bytes of path data]

08 00 00 00                          # Path 3: 8 bytes
[8 bytes of path data]

[32 bytes: Blake2b checksum]         # Checksum of everything above
```

---

## Paths Format Specification

### Format Layout

```
┌───────────────────────────────────────────────────────────────┐
│ MORK Paths Format (Compressed)                                │
├───────────────────────────────────────────────────────────────┤
│ HEADER (16 bytes)                                             │
│   [0-7]:   Magic "PATHMAP1"                                   │
│   [8-9]:   Version (u16 LE)                                   │
│   [10-11]: Compression flags (u16 LE)                         │
│      0x00: No compression                                     │
│      0x01: zlib                                               │
│      0x02: lz4                                                │
│      0x03: zstd                                               │
│   [12-15]: Reserved                                           │
├───────────────────────────────────────────────────────────────┤
│ COMPRESSED DATA                                               │
│   [0-7]:   Number of paths (u64 LE)                          │
│   For each path:                                             │
│     [0-3]:   Path length (u32 LE)                            │
│     [4-N]:   Path data                                       │
├───────────────────────────────────────────────────────────────┤
│ CHECKSUM                                                      │
│   [0-31]:  Blake2b-256 checksum                              │
└───────────────────────────────────────────────────────────────┘
```

### Compression Algorithms

```rust
pub enum CompressionAlgorithm {
    None = 0x00,
    Zlib = 0x01,
    Lz4 = 0x02,
    Zstd = 0x03,
}

pub fn compress_paths(data: &[u8], algorithm: CompressionAlgorithm) -> io::Result<Vec<u8>> {
    match algorithm {
        CompressionAlgorithm::None => Ok(data.to_vec()),
        CompressionAlgorithm::Zlib => {
            use flate2::write::ZlibEncoder;
            use flate2::Compression;

            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data)?;
            encoder.finish()
        }
        CompressionAlgorithm::Lz4 => {
            use lz4::EncoderBuilder;

            let mut encoder = EncoderBuilder::new().build(Vec::new())?;
            encoder.write_all(data)?;
            let (output, result) = encoder.finish();
            result?;
            Ok(output)
        }
        CompressionAlgorithm::Zstd => {
            use zstd::stream::write::Encoder;

            let mut encoder = Encoder::new(Vec::new(), 6)?;  // Level 6
            encoder.write_all(data)?;
            encoder.finish()
        }
    }
}
```

---

## ACT Format Specification

### Format Layout

```
┌───────────────────────────────────────────────────────────────┐
│ ArenaCompactTree Format v3                                    │
├───────────────────────────────────────────────────────────────┤
│ HEADER (64 bytes)                                             │
│   [0-11]:  Magic "ACTree03" + null padding                    │
│   [12-15]: Version (u32 LE) = 3                              │
│   [16-19]: Flags (u32 LE)                                    │
│   [20-27]: Root node offset (u64 LE)                         │
│   [28-35]: Total nodes (u64 LE)                              │
│   [36-43]: Value count (u64 LE)                              │
│   [44-63]: Reserved (20 bytes)                               │
├───────────────────────────────────────────────────────────────┤
│ NODE DATA                                                     │
│   For each node (24 bytes fixed):                            │
│     [0]:     Tag/arity (u8)                                   │
│     [1-3]:   Reserved (3 bytes)                              │
│     [4-7]:   Children offset (i32 LE, relative)              │
│     [8-15]:  Value (u64 LE)                                  │
│     [16-23]: Metadata (u64 LE)                               │
└───────────────────────────────────────────────────────────────┘
```

### Node Structure

```rust
#[repr(C)]
pub struct ActNode {
    pub tag: u8,              // Tag from BTM encoding (0-255)
    _reserved: [u8; 3],       // Alignment padding
    pub children_offset: i32, // Relative offset to children (-2^31 to 2^31-1)
    pub value: u64,           // Associated value (0 = no value)
    pub metadata: u64,        // Path count, depth, etc.
}

impl ActNode {
    pub const SIZE: usize = 24;

    pub fn has_value(&self) -> bool {
        self.value != 0
    }

    pub fn children_count(&self) -> usize {
        // Decode from tag based on BTM encoding rules
        match self.tag {
            0..=239 => (self.tag as usize) + 1,  // Arity 1-240
            240..=255 => 0,  // Special tags
        }
    }
}
```

### Memory Layout Example

For a tree with 3 nodes:
```
Offset 0x0000: Header (64 bytes)
41 43 54 72 65 65 30 33 00 00 00 00  # "ACTree03" + padding
03 00 00 00                           # Version 3
00 00 00 00                           # Flags 0
40 00 00 00 00 00 00 00              # Root at offset 0x40 (64)
03 00 00 00 00 00 00 00              # 3 nodes
02 00 00 00 00 00 00 00              # 2 values
[20 bytes reserved]

Offset 0x0040: Node 0 (root)
02                                     # Tag: arity 3
00 00 00                              # Reserved
18 00 00 00                           # Children at +24 bytes
00 00 00 00 00 00 00 00              # No value
01 00 00 00 00 00 00 00              # Metadata

Offset 0x0058: Node 1
F0                                     # Tag: symbol
00 00 00                              # Reserved
00 00 00 00                           # No children
01 00 00 00 00 00 00 00              # Value = 1
00 00 00 00 00 00 00 00              # Metadata

Offset 0x0070: Node 2
F0                                     # Tag: symbol
00 00 00                              # Reserved
00 00 00 00                           # No children
02 00 00 00 00 00 00 00              # Value = 2
00 00 00 00 00 00 00 00              # Metadata
```

---

## Symbol Table Format

### ZIP Archive Structure

```
symbol_table.zip
├── metadata.bin          # Metadata about all files
├── str_to_sym.bin        # String → Symbol ID mappings
├── sym_to_str.bin        # Symbol ID → String mappings
├── short_str_to_sym.bin  # Short strings (≤ 7 bytes)
└── sym_to_short_str.bin  # Short string reverse mappings
```

### metadata.bin Format

```
┌───────────────────────────────────────────┐
│ [0-7]:   str_to_sym file size (u64 LE)   │
│ [8-15]:  sym_to_str file size (u64 LE)   │
│ [16-23]: short_str_to_sym size (u64 LE)  │
│ [24-31]: sym_to_short_str size (u64 LE)  │
│ [32-39]: Max symbol ID (u64 LE)          │
│ [40-47]: Total symbols (u64 LE)          │
└───────────────────────────────────────────┘
```

### str_to_sym.bin Format

```
┌─────────────────────────────────────────────────────┐
│ [0-7]:   Entry count (u64 LE)                      │
│ For each entry:                                     │
│   [0]:     Symbol ID encoding flag                 │
│      0xFF: 8-byte symbol ID follows                │
│      other: 1-byte symbol ID (0-254)              │
│   [1-N]:   Symbol ID (1 or 8 bytes)                │
│   [N+1-N+4]: String length (u32 LE)                │
│   [N+5...]: UTF-8 string data                      │
└─────────────────────────────────────────────────────┘
```

### Variable-Length Symbol ID Encoding

```rust
fn encode_symbol_id(id: u64) -> Vec<u8> {
    if id <= 254 {
        vec![id as u8]
    } else {
        let mut bytes = vec![0xFF];
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes
    }
}

fn decode_symbol_id<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut flag = [0u8; 1];
    reader.read_exact(&mut flag)?;

    if flag[0] == 0xFF {
        let mut bytes = [0u8; 8];
        reader.read_exact(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    } else {
        Ok(flag[0] as u64)
    }
}
```

---

## Multiplicities Format

### Layout

```
┌───────────────────────────────────────────────────┐
│ MORK Multiplicities Format                       │
├───────────────────────────────────────────────────┤
│ HEADER (16 bytes)                                 │
│   [0-3]:   Magic "MTTM"                          │
│   [4-5]:   Version (u16 LE)                      │
│   [6-7]:   Flags (u16 LE)                        │
│   [8-15]:  Reserved                              │
├───────────────────────────────────────────────────┤
│ ENTRIES                                           │
│   [0-7]:   Entry count (u64 LE)                  │
│   For each entry:                                 │
│     [0-3]:   Key length (u32 LE)                 │
│     [4-N]:   Key (UTF-8 string)                  │
│     [N+1-N+8]: Value (u64 LE)                    │
└───────────────────────────────────────────────────┘
```

---

## Versioning Strategy

### Version Numbering

```
Version format: MAJOR.MINOR
- MAJOR: Incompatible changes (breaking)
- MINOR: Compatible additions (non-breaking)
```

### Supported Versions

| Version | Status | Features |
|---------|--------|----------|
| 1.0 | Current | Basic binary format, checksums |
| 1.1 | Planned | Compression flags, metadata |
| 2.0 | Future | Incremental serialization, streaming |

### Version Detection

```rust
pub fn detect_version(bytes: &[u8]) -> Result<u16, VersionError> {
    if bytes.len() < 6 {
        return Err(VersionError::TooShort);
    }

    // Check magic
    if &bytes[0..4] != b"MTTS" {
        return Err(VersionError::InvalidMagic);
    }

    // Read version
    Ok(u16::from_le_bytes([bytes[4], bytes[5]]))
}

pub fn can_deserialize(version: u16) -> bool {
    match version {
        1 => true,   // Current version
        2 => false,  // Future version (reject)
        _ => false,  // Unknown version
    }
}
```

---

## Schema Evolution

### Adding Fields (Minor Version)

**Example**: Adding timestamp to header

```rust
// Version 1.0 header
struct HeaderV1 {
    magic: [u8; 4],
    version: u16,
    flags: u16,
    reserved: [u8; 8],
}

// Version 1.1 header (use reserved space)
struct HeaderV11 {
    magic: [u8; 4],
    version: u16,
    flags: u16,
    timestamp: u64,  // Use first 8 reserved bytes
}

// Reading
fn read_header(bytes: &[u8]) -> Header {
    let version = detect_version(bytes)?;

    match version {
        1 => {
            // v1.0: No timestamp
            HeaderV1::from_bytes(bytes)
        }
        2 => {
            // v1.1: Has timestamp
            HeaderV11::from_bytes(bytes)
        }
    }
}
```

### Removing Fields (Major Version)

**Example**: Removing flags field

```rust
// Version 1.0
struct HeaderV1 {
    magic: [u8; 4],
    version: u16,
    flags: u16,      // Present
    reserved: [u8; 8],
}

// Version 2.0 (breaking change)
struct HeaderV2 {
    magic: [u8; 4],
    version: u16,
    // flags removed
    reserved: [u8; 10],  // Expanded reserved space
}

// Migration
fn migrate_v1_to_v2(v1_bytes: &[u8]) -> Vec<u8> {
    let header_v1 = HeaderV1::from_bytes(v1_bytes);

    // Create v2 header (discard flags)
    let header_v2 = HeaderV2 {
        magic: header_v1.magic,
        version: 2,
        reserved: [0; 10],
    };

    // ... copy rest of data
}
```

---

## Summary

This document specifies:

1. **Binary Format**: Complete byte-level layout for MORK spaces
2. **Paths Format**: Compressed path storage with multiple algorithms
3. **ACT Format**: Memory-mapped tree structure for instant loading
4. **Symbol Table**: ZIP-based interning with variable-length encoding
5. **Multiplicities**: Simple key-value format
6. **Versioning**: Strategy for backward and forward compatibility
7. **Schema Evolution**: Guidelines for adding/removing fields

All formats include:
- Magic numbers for identification
- Version fields for compatibility
- Checksums for integrity
- Reserved space for future extensions

---

**Document Version**: 1.0
**Last Updated**: 2025-11-13
