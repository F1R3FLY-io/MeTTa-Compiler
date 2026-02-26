# 02 — Binary File Format Specification (`.mtrace`)

**Source**: `src/backend/trace/format.rs`, `trace-format/src/lib.rs`

## File Layout

```
Offset   Size     Description
──────── ──────── ─────────────────────────────────────────────────
0x0000   8 bytes  Magic: "MTRACE\x00\x01"
0x0008   4 bytes  Header length (u32 LE)
0x000C   N bytes  Header (postcard-serialized TraceHeader)
         ╎        ╎
         ╎        ╎ ─── Events Region ───
         ╎        ╎
var      4 bytes  Event 0 length (u32 LE)
var      M bytes  Event 0 (postcard-serialized TraceEvent)
var      4 bytes  Event 1 length (u32 LE)
var      M bytes  Event 1 (postcard-serialized TraceEvent)
         ...      ...
         ╎        ╎
         ╎        ╎ ─── Footer ───
         ╎        ╎
var      4 bytes  Footer sentinel: 0x00000000 (u32 LE = 0)
var      8 bytes  Event count (u64 LE)
var      4 bytes  File table length (u32 LE)
var      K bytes  File table (postcard-serialized Vec<String>)
```

## Magic Bytes

```rust
pub const TRACE_MAGIC: [u8; 8] = *b"MTRACE\x00\x01";
```

The first 8 bytes of every `.mtrace` file. The `\x01` byte encodes the
format version. Readers must verify the magic bytes before proceeding.

## Format Version

```rust
pub const TRACE_FORMAT_VERSION: u32 = 1;
```

The current version. Embedded in the magic bytes (byte 7 = `0x01`).

## Header

Written by `format::write_header()` (`src/backend/trace/format.rs:22`).

1. Write 8-byte magic
2. Serialize `TraceHeader` with postcard
3. Write the serialized length as `u32 LE`
4. Write the serialized bytes

The header's `file_table` field is always empty at write time — the
actual file table is written in the footer (because file paths are
interned during event emission and the full set is not known until
finalization).

## Events

Written by `format::write_event()` (`src/backend/trace/format.rs:41`).

Each event is length-prefixed:

```
┌────────────────┬──────────────────────────┐
│ u32 LE length  │ postcard-serialized      │
│ (4 bytes)      │ TraceEvent (N bytes)     │
└────────────────┴──────────────────────────┘
```

Events are written in the order they are flushed from thread-local
buffers. Within a single thread, events are ordered by their `seq`
number. Across threads, events are interleaved in buffer-flush order.

### Streaming Writes

Events can be written incrementally — the writer does not need to know
the total event count upfront. This is critical for long-running
evaluations where the trace file may be multiple gigabytes.

## Footer

Written by `format::write_footer()` (`src/backend/trace/format.rs:55`).

The footer consists of three parts:

### 1. Sentinel (4 bytes)

```
u32 LE = 0
```

A zero-length event marker. Since no valid postcard-serialized
`TraceEvent` has zero length, the reader uses this as a stop signal.

### 2. Event Count (8 bytes)

```
u64 LE = total number of events written
```

Allows the reader to verify completeness and display progress.

### 3. File Table

```
u32 LE = file table serialized length
postcard-serialized Vec<String>
```

The file table maps `u16` file IDs (used in `TraceSpan::file_id`) to
file path strings. During event emission, `FileTable::intern()` assigns
monotonically increasing IDs starting from 0. The table is serialized
as a `Vec<String>` where the index is the file ID.

## Postcard Serialization

[Postcard](https://docs.rs/postcard) is a `#[no_std]`-compatible,
serde-based binary format that uses varint encoding for integers and
handles recursive types naturally.

Key properties:

- **Varint encoding**: Small integers use fewer bytes (e.g., `42` = 1 byte,
  not 8). This is significant since `seq`, `depth`, `thread_id` are
  typically small numbers.
- **Recursive types**: `TraceValue::SExpr(Vec<TraceValue>)` and
  `TraceValue::Error(String, Box<TraceValue>)` serialize without issue.
  This was the primary reason postcard was chosen over rkyv (see
  [`docs/design/eval-trace/serialization-format.md`](../../design/eval-trace/serialization-format.md)).
- **Compact**: No alignment padding, no schema overhead per record.
- **Serde ecosystem**: Standard `#[derive(Serialize, Deserialize)]`.

## Reading

The `trace-analyzer` uses memory-mapped I/O (`memmap2`) for zero-copy
access to the trace file.

**Source**: `tools/trace-analyzer/src/reader.rs`

`TraceReader::open()`:
1. Memory-map the file
2. Verify 8-byte magic
3. Read header length at offset 8
4. Deserialize `TraceHeader` from `[12..12+header_len]`
5. Locate footer: read file table offset from the last 12 bytes
6. Deserialize `Vec<String>` file table from footer

`TraceReader::events()` returns an `EventIterator` that:
1. Starts at `events_start` (immediately after header)
2. Reads `u32 LE` length prefix
3. Deserializes `TraceEvent` from the next `length` bytes
4. Advances offset
5. Stops when it encounters the zero sentinel or reaches the footer region

### Streaming Reads

Events can be consumed without loading the entire file into memory.
The memory-mapped approach provides the OS page cache with the
opportunity to manage physical memory efficiently for large traces.
