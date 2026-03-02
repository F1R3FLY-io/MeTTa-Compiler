//! Binary trace file format I/O (format v2).
//!
//! File layout:
//! ```text
//! [Magic: "MTRACE\x00\x02" (8 bytes)]
//! [Header length: u32 LE]
//! [Header: bitcode-serialized TraceHeader]
//! [Event 0: u32 LE length + bitcode bytes]
//! [Event 1: u32 LE length + bitcode bytes]
//! ...
//! [Footer marker: u32 LE = 0 (sentinel — no event has zero length)]
//! [Event count: u64 LE]
//! [File table length: u32 LE]
//! [File table: bitcode-serialized Vec<String>]
//! ```

use std::io::Write;

use trace_format::{TraceEvent, TraceHeader, TRACE_MAGIC};

/// Write the trace file header (magic + serialized TraceHeader).
pub fn write_header<W: Write>(
    writer: &mut W,
    header: &TraceHeader,
) -> std::io::Result<()> {
    // Magic bytes.
    writer.write_all(&TRACE_MAGIC)?;

    // Serialize header.
    let header_bytes = trace_format::serialize(header);

    // Header length prefix.
    let len = header_bytes.len() as u32;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&header_bytes)?;

    Ok(())
}

/// Write a single length-prefixed event.
pub fn write_event<W: Write>(
    writer: &mut W,
    event: &TraceEvent,
) -> std::io::Result<()> {
    let event_bytes = trace_format::serialize(event);

    let len = event_bytes.len() as u32;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&event_bytes)?;

    Ok(())
}

/// Write the footer: sentinel, event count, and file table.
pub fn write_footer<W: Write>(
    writer: &mut W,
    event_count: u64,
    file_table: &[String],
) -> std::io::Result<()> {
    // Sentinel (zero-length event marker — readers stop here).
    writer.write_all(&0u32.to_le_bytes())?;

    // Event count.
    writer.write_all(&event_count.to_le_bytes())?;

    // File table.
    let table_bytes = trace_format::serialize(&file_table.to_vec());
    let table_len = table_bytes.len() as u32;
    writer.write_all(&table_len.to_le_bytes())?;
    writer.write_all(&table_bytes)?;

    Ok(())
}
