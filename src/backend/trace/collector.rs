//! Thread-safe trace event collector with batched I/O.
//!
//! Each thread accumulates events in a thread-local buffer. When the
//! buffer reaches `BATCH_SIZE` events, or when `finalize()` is called,
//! the batch is serialized via rkyv and written to the shared output
//! file under a mutex.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use trace_format::{
    TraceEvent, TraceEventKind, TraceHeader, TraceSpan, TraceTier, TraceValue,
};

use super::convert::{trace_value, FileTable};
use super::format;
use crate::backend::models::metta_value::MettaValue;

/// Number of events to buffer per-thread before flushing.
const BATCH_SIZE: usize = 1024;

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
            events: Vec::with_capacity(BATCH_SIZE),
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
        };

        format::write_header(&mut writer, &header)?;

        let collector = Arc::new(Self {
            shared: Mutex::new(SharedState {
                writer,
                file_table: FileTable::new(),
                event_count: 0,
            }),
            start: Instant::now(),
            global_seq: AtomicU64::new(0),
            next_thread_id: AtomicU64::new(0),
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
            };

            buf.events.push(event);

            if buf.events.len() >= BATCH_SIZE {
                let batch = std::mem::replace(
                    &mut buf.events,
                    Vec::with_capacity(BATCH_SIZE),
                );
                // Drop the RefCell borrow before locking the mutex.
                drop(buf_opt);
                self.flush_batch(batch);
            }
        });
    }

    /// Intern a file path and return its `u16` ID (for span conversion).
    pub fn intern_file(&self, path: &str) -> u16 {
        let mut shared = self.shared.lock().expect("TraceCollector shared lock poisoned");
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
                let shared = arc.shared.lock().expect("TraceCollector shared lock poisoned");
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
        let mut shared = self.shared.lock().expect("TraceCollector shared lock poisoned");
        for event in &batch {
            if let Err(e) = format::write_event(&mut shared.writer, event) {
                eprintln!("[trace] Failed to write event: {e}");
                return;
            }
            shared.event_count += 1;
        }
    }
}
