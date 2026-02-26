//! Streaming trace file reader.
//!
//! Reads MeTTaTron binary trace files using memory-mapped I/O for zero-copy
//! access. Events are deserialized on demand via postcard.

use std::path::Path;

use memmap2::Mmap;
use trace_format::{
    TraceEvent, TraceHeader, TRACE_MAGIC,
};

/// A memory-mapped trace file with lazy event iteration.
pub struct TraceReader {
    mmap: Mmap,
    /// Byte offset where events begin (after header).
    events_start: usize,
    /// Parsed header.
    pub header: TraceHeader,
    /// File table from footer (populated after reading footer).
    pub file_table: Vec<String>,
}

impl TraceReader {
    /// Open and memory-map a trace file.
    pub fn open(path: &str) -> Result<Self, String> {
        let p = Path::new(path);
        if !p.exists() {
            return Err(format!("File not found: {path}"));
        }

        let file = std::fs::File::open(p)
            .map_err(|e| format!("Failed to open file: {e}"))?;

        let mmap = unsafe { Mmap::map(&file) }
            .map_err(|e| format!("Failed to mmap file: {e}"))?;

        if mmap.len() < 8 {
            return Err("File too small to contain trace header".to_string());
        }

        // Verify magic bytes
        if &mmap[0..8] != TRACE_MAGIC {
            return Err("Invalid trace file: magic bytes mismatch".to_string());
        }

        // Read header length (u32 LE at offset 8)
        if mmap.len() < 12 {
            return Err("File too small for header length".to_string());
        }
        let header_len = u32::from_le_bytes([mmap[8], mmap[9], mmap[10], mmap[11]]) as usize;

        if mmap.len() < 12 + header_len {
            return Err("File truncated: header extends past EOF".to_string());
        }

        let header_bytes = &mmap[12..12 + header_len];
        let header: TraceHeader = postcard::from_bytes(header_bytes)
            .map_err(|e| format!("Failed to decode header: {e}"))?;

        let events_start = 12 + header_len;

        // Try to read footer (last 12 bytes: u64 event_count + u32 file_table_offset)
        let file_table = if mmap.len() >= events_start + 12 {
            let footer_start = mmap.len() - 12;
            let ft_offset = u32::from_le_bytes([
                mmap[footer_start + 8],
                mmap[footer_start + 9],
                mmap[footer_start + 10],
                mmap[footer_start + 11],
            ]) as usize;

            if ft_offset < mmap.len() && ft_offset >= events_start {
                let ft_bytes = &mmap[ft_offset..footer_start];
                postcard::from_bytes::<Vec<String>>(ft_bytes).unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Ok(Self {
            mmap,
            events_start,
            header,
            file_table,
        })
    }

    /// Iterate over all events in the trace file.
    pub fn events(&self) -> EventIterator<'_> {
        // Determine the end of events region (before footer)
        let events_end = if self.mmap.len() >= self.events_start + 12 {
            let footer_start = self.mmap.len() - 12;
            let ft_offset = u32::from_le_bytes([
                self.mmap[footer_start + 8],
                self.mmap[footer_start + 9],
                self.mmap[footer_start + 10],
                self.mmap[footer_start + 11],
            ]) as usize;
            if ft_offset >= self.events_start {
                ft_offset
            } else {
                footer_start
            }
        } else {
            self.mmap.len()
        };

        EventIterator {
            data: &self.mmap,
            offset: self.events_start,
            end: events_end,
        }
    }

    /// Resolve a file_id to a path string.
    pub fn resolve_file(&self, file_id: u16) -> &str {
        self.file_table.get(file_id as usize)
            .map(|s| s.as_str())
            .unwrap_or("<unknown>")
    }
}

/// Iterator over trace events in a memory-mapped file.
pub struct EventIterator<'a> {
    data: &'a [u8],
    offset: usize,
    end: usize,
}

impl<'a> Iterator for EventIterator<'a> {
    type Item = TraceEvent;

    fn next(&mut self) -> Option<TraceEvent> {
        if self.offset + 4 > self.end {
            return None;
        }

        // Read event length (u32 LE)
        let len = u32::from_le_bytes([
            self.data[self.offset],
            self.data[self.offset + 1],
            self.data[self.offset + 2],
            self.data[self.offset + 3],
        ]) as usize;
        self.offset += 4;

        if self.offset + len > self.end {
            return None; // truncated
        }

        let event_bytes = &self.data[self.offset..self.offset + len];
        self.offset += len;

        postcard::from_bytes(event_bytes).ok()
    }
}
