//! Streaming trace file reader.
//!
//! Reads MeTTaTron binary trace files using memory-mapped I/O for zero-copy
//! access. Events are deserialized on demand via postcard.
//!
//! Handles crash-truncated files gracefully: if the process segfaulted before
//! writing the footer, the reader degrades to treating all bytes after the
//! header as events, and sets `truncated = true`.

use std::path::Path;

use memmap2::Mmap;
use trace_format::{
    TraceEvent, TraceHeader, TRACE_MAGIC, TRACE_MAGIC_V1, TRACE_MAGIC_V2, TRACE_MAGIC_V3,
    TRACE_MAGIC_V4,
};

/// Detect the footer in a trace file using backward sentinel scan.
///
/// Footer layout (from `format.rs`):
/// ```text
/// [Sentinel: u32 LE = 0]          — 4 bytes
/// [Event count: u64 LE]           — 8 bytes
/// [File table length: u32 LE]     — 4 bytes
/// [File table data: ft_len bytes] — variable (postcard Vec<String>)
/// ```
///
/// Returns `(file_table, events_end, truncated)`.
/// - `events_end`: byte offset where events stop (= sentinel position if found, else EOF)
/// - `truncated`: true if no valid footer was found (crash-truncated file)
fn detect_footer(mmap: &[u8], events_start: usize) -> (Vec<String>, usize, bool) {
    // Minimum footer: sentinel(4) + event_count(8) + ft_len(4) + empty_postcard_vec(1) = 17 bytes
    let min_footer = 17;
    if mmap.len() < events_start + min_footer {
        return (Vec::new(), mmap.len(), true);
    }

    // Scan backward from the latest possible sentinel position.
    // Sentinel is at: mmap.len() - 16 - ft_len
    // Latest: ft_len = 1 (minimum postcard empty Vec) → pos = mmap.len() - 17
    // Earliest: pos = events_start (no events at all)
    let scan_start = mmap.len() - min_footer;
    let scan_end = events_start;

    let mut pos = scan_start;
    loop {
        // Check for sentinel (0u32)
        let sentinel = u32::from_le_bytes([mmap[pos], mmap[pos + 1], mmap[pos + 2], mmap[pos + 3]]);
        if sentinel == 0 && pos + 16 <= mmap.len() {
            let ft_len = u32::from_le_bytes([
                mmap[pos + 12],
                mmap[pos + 13],
                mmap[pos + 14],
                mmap[pos + 15],
            ]) as usize;

            // Validate: file table must reach exactly EOF
            if pos + 16 + ft_len == mmap.len() {
                // Try to deserialize file table
                let ft_bytes = &mmap[pos + 16..mmap.len()];
                if let Ok(file_table) = postcard::from_bytes::<Vec<String>>(ft_bytes) {
                    return (file_table, pos, false);
                }
            }
        }

        if pos == scan_end {
            break;
        }
        pos -= 1;
    }

    // No valid footer found — crash-truncated file
    (Vec::new(), mmap.len(), true)
}

/// A memory-mapped trace file with lazy event iteration.
pub struct TraceReader {
    mmap: Mmap,
    /// Byte offset where events begin (after header).
    events_start: usize,
    /// Byte offset where events end (sentinel position if footer present, else EOF).
    events_end: usize,
    /// Parsed header.
    pub header: TraceHeader,
    /// File table from footer (populated after reading footer).
    pub file_table: Vec<String>,
    /// Detected format version (1 or 2). v1 files lack duration_ns/span_id.
    pub format_version: u32,
    /// True if the file appears to be crash-truncated (no valid footer found).
    pub truncated: bool,
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

        // Verify magic bytes (accept v1..v5)
        let format_version = if mmap[0..8] == TRACE_MAGIC {
            5u32
        } else if mmap[0..8] == TRACE_MAGIC_V4 {
            4u32
        } else if mmap[0..8] == TRACE_MAGIC_V3 {
            3u32
        } else if mmap[0..8] == TRACE_MAGIC_V2 {
            2u32
        } else if mmap[0..8] == TRACE_MAGIC_V1 {
            1u32
        } else {
            return Err("Invalid trace file: magic bytes mismatch".to_string());
        };

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

        // Detect footer using backward sentinel scan
        let (file_table, events_end, truncated) = detect_footer(&mmap, events_start);

        Ok(Self {
            mmap,
            events_start,
            events_end,
            header,
            file_table,
            format_version,
            truncated,
        })
    }

    /// Iterate over all events in the trace file.
    pub fn events(&self) -> EventIterator<'_> {
        EventIterator {
            data: &self.mmap,
            offset: self.events_start,
            end: self.events_end,
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

        // Zero-length sentinel marks end of events (footer follows)
        if len == 0 {
            return None;
        }

        if self.offset + len > self.end {
            return None; // truncated
        }

        let event_bytes = &self.data[self.offset..self.offset + len];
        self.offset += len;

        postcard::from_bytes(event_bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_header() -> TraceHeader {
        TraceHeader {
            source_file: "test.metta".to_string(),
            start_time_ns: 1000,
            mettatron_version: "0.1.0".to_string(),
            cpu_count: 4,
            file_table: vec![],
            format_version: 2,
        }
    }

    /// Build a minimal well-formed trace file in memory.
    fn build_trace_file(events: &[&[u8]], file_table: &[String]) -> Vec<u8> {
        let mut buf = Vec::new();

        // Magic (v2)
        buf.extend_from_slice(&TRACE_MAGIC);

        // Header
        let header = test_header();
        let header_bytes = postcard::to_allocvec(&header).expect("header serialize");
        buf.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&header_bytes);

        // Events (raw pre-serialized bytes with length prefix)
        for event_data in events {
            buf.extend_from_slice(&(event_data.len() as u32).to_le_bytes());
            buf.extend_from_slice(event_data);
        }

        // Footer: sentinel + event_count + ft_len + ft_data
        buf.extend_from_slice(&0u32.to_le_bytes()); // sentinel
        buf.extend_from_slice(&(events.len() as u64).to_le_bytes()); // event count
        let ft_bytes = postcard::to_allocvec(&file_table.to_vec()).expect("ft serialize");
        buf.extend_from_slice(&(ft_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&ft_bytes);

        buf
    }

    #[test]
    fn test_detect_footer_well_formed() {
        let file_table = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        // Use some non-zero fake event data
        let fake_event: &[u8] = &[1, 2, 3, 4, 5];
        let data = build_trace_file(&[fake_event, fake_event], &file_table);

        // events_start is after magic(8) + header_len(4) + header_bytes
        let header_bytes = postcard::to_allocvec(&test_header()).expect("header serialize");
        let events_start = 12 + header_bytes.len();

        let (ft, events_end, truncated) = detect_footer(&data, events_start);
        assert!(!truncated, "well-formed file should not be truncated");
        assert_eq!(ft.len(), 2);
        assert_eq!(ft[0], "src/main.rs");
        assert_eq!(ft[1], "src/lib.rs");
        assert!(events_end < data.len(), "events_end should be before EOF");
    }

    #[test]
    fn test_detect_footer_truncated() {
        // Build a file with header + events but NO footer (simulating crash)
        let mut buf = Vec::new();

        // Magic
        buf.extend_from_slice(&TRACE_MAGIC);

        // Header
        let header = TraceHeader {
            source_file: "test.metta".to_string(),
            start_time_ns: 1000,
            mettatron_version: "0.1.0".to_string(),
            cpu_count: 4,
            file_table: vec![],
            format_version: 2,
        };
        let header_bytes = postcard::to_allocvec(&header).expect("header serialize");
        buf.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&header_bytes);

        // Some raw event data (no footer)
        let fake_event: &[u8] = &[10, 20, 30, 40, 50];
        buf.extend_from_slice(&(fake_event.len() as u32).to_le_bytes());
        buf.extend_from_slice(fake_event);

        let events_start = 12 + header_bytes.len();
        let (ft, events_end, truncated) = detect_footer(&buf, events_start);
        assert!(truncated, "file without footer should be truncated");
        assert!(ft.is_empty(), "truncated file should have empty file table");
        assert_eq!(events_end, buf.len(), "events_end should be EOF for truncated file");
    }

    #[test]
    fn test_detect_footer_empty_file_table() {
        let file_table: Vec<String> = vec![];
        let fake_event: &[u8] = &[7, 8, 9];
        let data = build_trace_file(&[fake_event], &file_table);

        let header_bytes = postcard::to_allocvec(&test_header()).expect("header serialize");
        let events_start = 12 + header_bytes.len();

        let (ft, events_end, truncated) = detect_footer(&data, events_start);
        assert!(!truncated);
        assert!(ft.is_empty());
        assert!(events_end < data.len());
    }
}
