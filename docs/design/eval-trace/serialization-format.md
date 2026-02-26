# Why Postcard Over rkyv

## Problem

The trace system needs a binary serialization format for `TraceEvent`
records. The format must handle recursive types like
`TraceValue::SExpr(Vec<TraceValue>)` and `TraceValue::Error(String,
Box<TraceValue>)`.

## Alternatives Considered

### rkyv (Zero-Copy Deserialization)

[rkyv](https://docs.rs/rkyv) is a zero-copy deserialization framework.
It writes data in a format that can be memory-mapped and accessed
directly without a deserialization step. This would make trace analysis
extremely fast — `TraceReader` could just `mmap` the file and cast
aligned pointers.

**Why it was rejected:**

rkyv requires that all types implement `Archive`, `Serialize`, and
`Deserialize` traits. These trait implementations work by computing
fixed memory layouts at compile time. Recursive types like
`TraceValue::SExpr(Vec<TraceValue>)` cause rkyv's derive macros to
generate code that overflows the stack during serialization of deeply
nested values.

This is a fundamental limitation of rkyv's approach: it needs to
compute a fixed layout for the archived type, but recursive types
have unbounded depth.

### bincode

bincode was the original candidate (the comments in `collector.rs`
still reference "bitcode" from an early iteration). bincode handles
recursive types but has been less actively maintained than postcard
and produces slightly larger output due to fixed-width integer encoding.

## Why Postcard Wins

[Postcard](https://docs.rs/postcard) is a `#[no_std]`-compatible,
serde-based binary format.

1. **Handles recursive types naturally**: Postcard uses standard serde
   `Serialize`/`Deserialize` traits. Recursive types like
   `Vec<TraceValue>` are serialized as length-prefixed sequences with
   no layout computation issues.

2. **Varint encoding**: Integers are encoded as variable-length bytes.
   Small values (common in traces — `seq`, `depth`, `thread_id`) use
   fewer bytes. A `u64` with value `42` uses 1 byte instead of 8.

3. **Actively maintained**: Postcard is actively developed by the
   embedded Rust community and has a stable API.

4. **Compact output**: No alignment padding, no schema overhead per
   record. A typical `TraceEvent` serializes to 50–200 bytes depending
   on the event kind and expression sizes.

5. **Serde ecosystem**: Standard `#[derive(Serialize, Deserialize)]`
   on all types. No custom trait implementations or derive macros
   beyond serde.

## Trade-Offs

| Aspect | Postcard | rkyv |
|--------|----------|------|
| Recursive types | Natural | Stack overflow |
| Read speed | Requires deserialization | Zero-copy (mmap + cast) |
| Write speed | ~100–300ns per event | Similar |
| Integer encoding | Varint (compact) | Fixed-width (aligned) |
| Maintenance | Active | Active |
| `#[no_std]` | Yes | Yes |

The main trade-off is that postcard requires a deserialization step when
reading events, while rkyv would allow zero-copy access. However:

- Trace analysis is an **offline** operation — it does not run during
  evaluation. Read speed is less critical than correctness.
- The `trace-analyzer` already uses memory-mapped I/O for the file
  data, so the deserialization overhead is the only cost.
- In practice, deserialization speed is bounded by memory bandwidth
  (reading the bytes from the page cache), not CPU.

## Implementation

### trace-format crate (`trace-format/Cargo.toml`)

```toml
[dependencies]
postcard = { version = "1", features = ["alloc"] }
serde = { version = "1", features = ["derive"] }
```

### Helper functions (`trace-format/src/lib.rs`)

```rust
pub fn serialize<T: serde::Serialize>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value)
        .expect("trace serialization should not fail")
}

pub fn deserialize<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}
```

### Usage in writer (`src/backend/trace/format.rs`)

```rust
let event_bytes = trace_format::serialize(event);
let len = event_bytes.len() as u32;
writer.write_all(&len.to_le_bytes())?;
writer.write_all(&event_bytes)?;
```

### Usage in reader (`tools/trace-analyzer/src/reader.rs`)

```rust
let event_bytes = &self.data[self.offset..self.offset + len];
postcard::from_bytes(event_bytes).ok()
```
