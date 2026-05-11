//! Thread-safe trace event collector with batched I/O.
//!
//! Each thread accumulates events in a thread-local buffer. When the
//! buffer reaches `TRACE_BATCH_SIZE` events, or when `finalize()` is called,
//! the batch is serialized via rkyv and written to the shared output
//! file under a mutex.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use trace_format::{TraceEvent, TraceEventKind, TraceHeader, TraceSpan, TraceTier, TraceValue};

use super::convert::{trace_value, FileTable};
use super::format;
use crate::backend::models::metta_value::MettaValue;

/// Number of events to buffer per thread before flushing.
const TRACE_BATCH_SIZE: usize = 64;

// ---------------------------------------------------------------------------
// Thread-local state
// ---------------------------------------------------------------------------

struct ThreadBuffer {
    events: Vec<TraceEvent>,
    seq: u64,
    thread_id: u32,
}

impl ThreadBuffer {
    fn new(thread_id: u32) -> Self {
        Self {
            events: Vec::with_capacity(TRACE_BATCH_SIZE),
            seq: 0,
            thread_id,
        }
    }
}

thread_local! {
    static THREAD_BUF: RefCell<Option<ThreadBuffer>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Shared output file + file table, protected by a mutex.
struct SharedState {
    writer: BufWriter<File>,
    file_table: FileTable,
    event_count: u64,
    /// Byte offset where the footer should be written. Updated after every
    /// batch flush so that the file always has a valid footer — even if the
    /// process is killed mid-evaluation. Initialized to the position right
    /// after the header.
    footer_start_pos: u64,
}

// ---------------------------------------------------------------------------
// TraceCollector
// ---------------------------------------------------------------------------

/// The main trace collector. Create one per evaluation session via
/// `TraceCollector::new()`, pass it through the `EvalContext`, and call
/// `finalize()` when evaluation completes.
pub struct TraceCollector {
    shared: Mutex<SharedState>,
    start: Instant,
    global_seq: AtomicU64,
    /// Next thread ID to assign.
    next_thread_id: AtomicU64,
    /// Monotonic span correlation ID generator.
    span_id_counter: AtomicU64,
}

impl TraceCollector {
    /// Create a new `TraceCollector` that writes to `path`.
    ///
    /// Writes the trace header immediately. The caller should pass
    /// `source_file` (the `.metta` file being evaluated).
    pub fn new(path: &str, source_file: &str) -> std::io::Result<Arc<Self>> {
        let file = File::create(path)?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);

        let header = TraceHeader {
            source_file: source_file.to_string(),
            start_time_ns: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            mettatron_version: env!("CARGO_PKG_VERSION").to_string(),
            cpu_count: num_cpus::get() as u32,
            file_table: Vec::new(), // Will be written in footer.
            format_version: trace_format::TRACE_FORMAT_VERSION,
        };

        format::write_header(&mut writer, &header)?;

        // Record the byte position right after the header — this is where
        // the first footer will be written (overwritten as events arrive).
        use std::io::Seek;
        let footer_start_pos = writer.stream_position().unwrap_or(0);

        // Write an initial footer so the file is valid even with 0 events.
        format::write_footer(&mut writer, 0, &[])?;
        writer.flush()?;

        let collector = Arc::new(Self {
            shared: Mutex::new(SharedState {
                writer,
                file_table: FileTable::new(),
                event_count: 0,
                footer_start_pos,
            }),
            start: Instant::now(),
            global_seq: AtomicU64::new(0),
            next_thread_id: AtomicU64::new(0),
            span_id_counter: AtomicU64::new(1), // Start at 1 so 0 is never a valid span ID
        });

        Ok(collector)
    }

    /// Emit a trace event.
    ///
    /// The `input` and `outputs` are live `MettaValue` references that
    /// will be converted to owned `TraceValue` snapshots immediately.
    pub fn emit(
        &self,
        tier: TraceTier,
        depth: u32,
        input: &MettaValue,
        outputs: &[MettaValue],
        expr_span: Option<TraceSpan>,
        kind: TraceEventKind,
    ) {
        let input_tv = trace_value(input);
        let outputs_tv: Vec<TraceValue> = outputs.iter().map(trace_value).collect();

        self.emit_converted(tier, depth, input_tv, outputs_tv, expr_span, kind);
    }

    /// Emit a trace event with pre-converted `TraceValue` data.
    ///
    /// Use this when you already have `TraceValue` instances (e.g. for
    /// lifecycle events like `EvalStart`/`EvalEnd` where input is Unit).
    pub fn emit_converted(
        &self,
        tier: TraceTier,
        depth: u32,
        input: TraceValue,
        outputs: Vec<TraceValue>,
        expr_span: Option<TraceSpan>,
        kind: TraceEventKind,
    ) {
        let timestamp_ns = self.start.elapsed().as_nanos() as u64;

        THREAD_BUF.with(|buf_cell| {
            let mut buf_opt = buf_cell.borrow_mut();
            let buf = buf_opt.get_or_insert_with(|| {
                let tid = self.next_thread_id.fetch_add(1, Ordering::Relaxed) as u32;
                ThreadBuffer::new(tid)
            });

            let seq = buf.seq;
            buf.seq += 1;

            let event = TraceEvent {
                seq,
                thread_id: buf.thread_id,
                timestamp_ns,
                tier,
                depth,
                input,
                outputs,
                expr_span,
                kind,
                duration_ns: None,
                span_id: None,
            };

            buf.events.push(event);

            if buf.events.len() >= TRACE_BATCH_SIZE {
                let batch =
                    std::mem::replace(&mut buf.events, Vec::with_capacity(TRACE_BATCH_SIZE));
                // Drop the RefCell borrow before locking the mutex.
                drop(buf_opt);
                self.flush_batch(batch);
            }
        });
    }

    /// Get elapsed nanoseconds since trace start.
    ///
    /// Use this to capture a start timestamp before an operation, then
    /// compute `duration_ns = elapsed_ns() - start_ns` after it completes.
    #[inline]
    pub fn elapsed_ns(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    /// Generate a globally unique span correlation ID.
    ///
    /// IDs are monotonically increasing and never zero, so `0` can serve
    /// as a sentinel for "no span".
    #[inline]
    pub fn next_span_id(&self) -> u64 {
        self.span_id_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Emit a timed trace event with explicit start time, duration, and
    /// optional span correlation ID.
    ///
    /// Use this for events that measure an operation's wall-clock duration.
    /// The `start_ns` becomes the event's `timestamp_ns` (so the event
    /// is anchored at the operation's *start*, not its end).
    pub fn emit_timed(
        &self,
        tier: TraceTier,
        depth: u32,
        input: TraceValue,
        outputs: Vec<TraceValue>,
        expr_span: Option<TraceSpan>,
        kind: TraceEventKind,
        start_ns: u64,
        duration_ns: Option<u64>,
        span_id: Option<u64>,
    ) {
        THREAD_BUF.with(|buf_cell| {
            let mut buf_opt = buf_cell.borrow_mut();
            let buf = buf_opt.get_or_insert_with(|| {
                let tid = self.next_thread_id.fetch_add(1, Ordering::Relaxed) as u32;
                ThreadBuffer::new(tid)
            });

            let seq = buf.seq;
            buf.seq += 1;

            let event = TraceEvent {
                seq,
                thread_id: buf.thread_id,
                timestamp_ns: start_ns,
                tier,
                depth,
                input,
                outputs,
                expr_span,
                kind,
                duration_ns,
                span_id,
            };

            buf.events.push(event);

            if buf.events.len() >= TRACE_BATCH_SIZE {
                let batch =
                    std::mem::replace(&mut buf.events, Vec::with_capacity(TRACE_BATCH_SIZE));
                drop(buf_opt);
                self.flush_batch(batch);
            }
        });
    }

    /// Intern a file path and return its `u16` ID (for span conversion).
    pub fn intern_file(&self, path: &str) -> u16 {
        let mut shared = self
            .shared
            .lock()
            .expect("TraceCollector shared lock poisoned");
        shared.file_table.intern(path)
    }

    /// Finalize the trace: flush remaining thread-local buffers, write
    /// footer with event count and file table.
    ///
    /// This **must** be called before the process exits to ensure all
    /// events are written. Consumes the `Arc<TraceCollector>` via
    /// `Arc::try_unwrap` (caller should be the last holder).
    pub fn finalize(self: Arc<Self>) -> std::io::Result<u64> {
        // Flush this thread's buffer.
        THREAD_BUF.with(|buf_cell| {
            let mut buf_opt = buf_cell.borrow_mut();
            if let Some(buf) = buf_opt.take() {
                if !buf.events.is_empty() {
                    self.flush_batch(buf.events);
                }
            }
        });

        // Try to unwrap the Arc; if other threads still hold references,
        // we just flush what we can.
        let collector = match Arc::try_unwrap(self) {
            Ok(c) => c,
            Err(arc) => {
                // Other threads may still hold references. Flush and return count.
                let shared = arc
                    .shared
                    .lock()
                    .expect("TraceCollector shared lock poisoned");
                return Ok(shared.event_count);
            }
        };

        let mut shared = collector
            .shared
            .into_inner()
            .expect("TraceCollector shared lock poisoned");

        let file_table_paths = shared.file_table.into_paths();
        let event_count = shared.event_count;

        format::write_footer(&mut shared.writer, event_count, &file_table_paths)?;
        shared.writer.flush()?;

        Ok(event_count)
    }

    /// Flush a batch of events to the shared writer.
    fn flush_batch(&self, batch: Vec<TraceEvent>) {
        use std::io::Seek;

        let mut shared = self
            .shared
            .lock()
            .expect("TraceCollector shared lock poisoned");

        // Seek back to overwrite the previous footer so this batch's events
        // start where the old footer was. This means the file always ends
        // with a valid footer — even if the process is killed before
        // finalize() runs.
        let seek_pos = shared.footer_start_pos;
        if let Err(e) = shared.writer.seek(std::io::SeekFrom::Start(seek_pos)) {
            eprintln!("[trace] Failed to seek to footer position: {e}");
            return;
        }

        // Write events
        for event in &batch {
            if let Err(e) = format::write_event(&mut shared.writer, event) {
                eprintln!("[trace] Failed to write event: {e}");
                return;
            }
            shared.event_count += 1;
        }

        // Record where the new footer starts
        shared.footer_start_pos = shared
            .writer
            .stream_position()
            .unwrap_or(shared.footer_start_pos);

        // Write footer (will be overwritten on next flush).
        // Clone file table paths to avoid borrowing shared immutably while
        // writer is borrowed mutably.
        let file_table: Vec<String> = shared.file_table.paths().to_vec();
        let event_count = shared.event_count;
        if let Err(e) = format::write_footer(&mut shared.writer, event_count, &file_table) {
            eprintln!("[trace] Failed to write footer: {e}");
            return;
        }

        // Ensure bytes are on disk (not just in BufWriter's internal buffer)
        if let Err(e) = shared.writer.flush() {
            eprintln!("[trace] Failed to flush writer: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_span_id_uniqueness() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_span_ids.mtrace");
        let tc = TraceCollector::new(path.to_str().expect("valid temp path"), "test.metta")
            .expect("create collector");

        let mut ids = HashSet::new();
        for _ in 0..1000 {
            let id = tc.next_span_id();
            assert!(ids.insert(id), "span ID {id} was not unique");
        }

        // IDs start at 1 and are monotonically increasing
        assert!(!ids.contains(&0), "span ID 0 should never be generated");
        assert!(ids.contains(&1), "first span ID should be 1");
        assert!(ids.contains(&1000), "last span ID should be 1000");

        let _ = tc.finalize();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_elapsed_ns_monotonic() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_elapsed_ns.mtrace");
        let tc = TraceCollector::new(path.to_str().expect("valid temp path"), "test.metta")
            .expect("create collector");

        let t1 = tc.elapsed_ns();
        // Spin briefly to ensure elapsed advances
        std::hint::spin_loop();
        let t2 = tc.elapsed_ns();

        assert!(
            t2 >= t1,
            "elapsed_ns should be monotonically non-decreasing: t1={t1}, t2={t2}"
        );

        let _ = tc.finalize();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_emit_timed_writes_duration_and_span() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_emit_timed.mtrace");
        let path_str = path.to_str().expect("valid temp path").to_string();

        {
            let tc = TraceCollector::new(&path_str, "test.metta").expect("create collector");

            let start = tc.elapsed_ns();
            let span = tc.next_span_id();

            // Emit a timed EvalStart
            tc.emit_timed(
                TraceTier::TreeWalker,
                0,
                TraceValue::Unit,
                vec![],
                None,
                TraceEventKind::EvalStart,
                start,
                None,
                Some(span),
            );

            // Simulate some work
            std::hint::spin_loop();
            let end = tc.elapsed_ns();
            let dur = end.saturating_sub(start);

            // Emit a timed EvalEnd with duration
            tc.emit_timed(
                TraceTier::TreeWalker,
                0,
                TraceValue::Unit,
                vec![TraceValue::Long(42)],
                None,
                TraceEventKind::EvalEnd { result_count: 1 },
                start,
                Some(dur),
                Some(span),
            );

            let count = tc.finalize().expect("finalize should succeed");
            assert_eq!(count, 2, "should have written exactly 2 events");
        }

        // Read back the trace file and verify events
        let data = std::fs::read(&path).expect("read trace file");
        // Skip the 8-byte magic + header length-prefix + header
        let mut pos = 8;
        // Read header length (4 bytes LE)
        let header_len =
            u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4 + header_len;

        // Read first event
        let ev1_len =
            u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        let ev1: TraceEvent =
            trace_format::deserialize(&data[pos..pos + ev1_len]).expect("deserialize event 1");
        pos += ev1_len;

        // Read second event
        let ev2_len =
            u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        let ev2: TraceEvent =
            trace_format::deserialize(&data[pos..pos + ev2_len]).expect("deserialize event 2");

        // Verify event 1 (EvalStart with span, no duration)
        assert!(matches!(ev1.kind, TraceEventKind::EvalStart));
        assert!(ev1.duration_ns.is_none());
        assert!(ev1.span_id.is_some());

        // Verify event 2 (EvalEnd with span AND duration)
        assert!(matches!(ev1.kind, TraceEventKind::EvalStart));
        assert!(ev2.duration_ns.is_some());
        assert!(ev2.span_id.is_some());

        // Both events should share the same span ID
        assert_eq!(
            ev1.span_id, ev2.span_id,
            "paired events should share span_id"
        );

        let _ = std::fs::remove_file(&path);
    }
}
