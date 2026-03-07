// gdb_parser.rs — Parse GDB `thread apply all bt` output into structured data.
//
// Parses thread headers, stack frames (with source locations, library references,
// signal handler markers), and identifies the crash thread.
//
// Uses manual string parsing (no regex crate) for zero additional dependencies.

use crate::function_map::{classify_rust_function, TraceCategory};

/// A single stack frame from a GDB backtrace.
#[derive(Debug, Clone)]
pub struct GdbFrame {
    /// Frame number (e.g., #0, #1, #2).
    pub frame_num: u32,
    /// Demangled function name (may be qualified with module path).
    pub function_name: String,
    /// Source file and line number, if available.
    pub source_location: Option<(String, u32)>,
    /// Shared library name, if the frame is from an external library.
    pub from_lib: Option<String>,
    /// True if this frame is `<signal handler called>`.
    pub is_signal_handler: bool,
    /// Semantic category of this frame's function.
    pub category: TraceCategory,
}

/// A single thread from a GDB backtrace.
#[derive(Debug, Clone)]
pub struct GdbThread {
    /// GDB's internal thread number (e.g., "Thread 15").
    pub gdb_thread_num: u32,
    /// OS lightweight process ID (LWP), if present.
    pub lwp: Option<u32>,
    /// Thread name (e.g., "mettatron", "mettatron-wk-0"), if present.
    pub thread_name: Option<String>,
    /// Stack frames, outermost (crash point) first.
    pub frames: Vec<GdbFrame>,
    /// True if this thread was identified as the crash thread.
    pub is_crash_thread: bool,
}

/// Complete GDB backtrace for all threads.
#[derive(Debug)]
pub struct GdbBacktrace {
    /// All threads in the backtrace.
    pub threads: Vec<GdbThread>,
    /// Index into `threads` of the crash thread, if identified.
    pub crash_thread_idx: Option<usize>,
}

/// Signal-related function names that indicate the crash point.
const SIGNAL_FUNCTIONS: &[&str] = &[
    "__restore_rt",
    "raise",
    "abort",
    "__GI_raise",
    "__GI_abort",
    "gsignal",
];

/// Panic-related function names that indicate a Rust panic.
const PANIC_FUNCTIONS: &[&str] = &[
    "__rust_start_panic",
    "rust_begin_unwind",
    "rust_panic",
    "std::panicking::begin_panic",
    "core::panicking::panic",
    "__rust_panic_cleanup",
];

/// Parse the complete GDB `thread apply all bt` output.
pub fn parse_gdb_backtrace(text: &str) -> Result<GdbBacktrace, String> {
    let mut threads: Vec<GdbThread> = Vec::new();
    let mut current_thread: Option<GdbThread> = None;

    for line in text.lines() {
        let trimmed = line.trim();

        // Skip empty lines and GDB prompt/info lines
        if trimmed.is_empty() || trimmed.starts_with("(gdb)") || trimmed.starts_with("---") {
            continue;
        }

        // Check for thread header
        if let Some(thread) = try_parse_thread_header(trimmed) {
            if let Some(prev) = current_thread.take() {
                threads.push(prev);
            }
            current_thread = Some(thread);
            continue;
        }

        // Check for frame line (starts with #N)
        if let Some(frame) = try_parse_frame(trimmed) {
            if let Some(ref mut thread) = current_thread {
                thread.frames.push(frame);
            }
            continue;
        }
    }

    // Push the last thread
    if let Some(thread) = current_thread {
        threads.push(thread);
    }

    if threads.is_empty() {
        return Err("No threads found in GDB backtrace output".to_string());
    }

    // Detect crash thread
    let crash_thread_idx = detect_crash_thread(&threads);

    // Mark the crash thread
    if let Some(idx) = crash_thread_idx {
        threads[idx].is_crash_thread = true;
    }

    Ok(GdbBacktrace {
        threads,
        crash_thread_idx,
    })
}

/// Try to parse a thread header line.
///
/// Format: `Thread N (Thread 0x... (LWP NNNNN) "name"):`
/// or:     `Thread N (LWP NNNNN):`
fn try_parse_thread_header(line: &str) -> Option<GdbThread> {
    // Must start with "Thread " and end with ":"
    if !line.starts_with("Thread ") || !line.ends_with(':') {
        return None;
    }

    // Extract thread number: "Thread N ..."
    let after_thread = &line["Thread ".len()..];
    let thread_num_end = after_thread.find(|c: char| !c.is_ascii_digit())?;
    let thread_num: u32 = after_thread[..thread_num_end].parse().ok()?;

    // Extract LWP: find "LWP " followed by digits
    let lwp = if let Some(lwp_start) = line.find("LWP ") {
        let after_lwp = &line[lwp_start + 4..];
        let lwp_end = after_lwp
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_lwp.len());
        after_lwp[..lwp_end].parse().ok()
    } else {
        None
    };

    // Extract thread name: find last quoted string
    let thread_name = extract_quoted_string(line);

    Some(GdbThread {
        gdb_thread_num: thread_num,
        lwp,
        thread_name,
        frames: Vec::new(),
        is_crash_thread: false,
    })
}

/// Extract the last double-quoted string from a line.
fn extract_quoted_string(line: &str) -> Option<String> {
    let last_quote_end = line.rfind('"')?;
    let before = &line[..last_quote_end];
    let quote_start = before.rfind('"')?;
    Some(line[quote_start + 1..last_quote_end].to_string())
}

/// Try to parse a stack frame line.
///
/// Formats:
/// - `#N  0xADDR in FUNC (ARGS) at FILE:LINE`
/// - `#N  0xADDR in FUNC (ARGS) from LIB`
/// - `#N  0xADDR in FUNC ()`
/// - `#N  <signal handler called>`
/// - `#N  FUNC (ARGS) at FILE:LINE`  (for inlined frames)
fn try_parse_frame(line: &str) -> Option<GdbFrame> {
    let trimmed = line.trim();

    // Must start with '#'
    if !trimmed.starts_with('#') {
        return None;
    }

    // Parse frame number
    let after_hash = &trimmed[1..];
    let num_end = after_hash
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_hash.len());
    if num_end == 0 {
        return None;
    }
    let frame_num: u32 = after_hash[..num_end].parse().ok()?;

    let rest = after_hash[num_end..].trim();

    // Check for <signal handler called>
    if rest.contains("<signal handler called>") {
        return Some(GdbFrame {
            frame_num,
            function_name: "<signal handler called>".to_string(),
            source_location: None,
            from_lib: None,
            is_signal_handler: true,
            category: TraceCategory::Other,
        });
    }

    // Extract function name, source location, and library
    let (function_name, source_location, from_lib) = parse_frame_details(rest);

    let category = classify_rust_function(&function_name);

    Some(GdbFrame {
        frame_num,
        function_name,
        source_location,
        from_lib,
        is_signal_handler: false,
        category,
    })
}

/// Parse the details portion of a frame line (after `#N  `).
///
/// Returns (function_name, source_location, from_lib).
fn parse_frame_details(rest: &str) -> (String, Option<(String, u32)>, Option<String>) {
    let mut work = rest;

    // Skip address prefix: "0xADDR in "
    if work.starts_with("0x") || work.starts_with("0X") {
        if let Some(in_pos) = work.find(" in ") {
            work = &work[in_pos + 4..];
        }
    }

    // Extract function name (up to first '(' or ' at ' or ' from ')
    let func_end = work
        .find(" (")
        .or_else(|| work.find('('))
        .or_else(|| work.find(" at "))
        .or_else(|| work.find(" from "))
        .unwrap_or(work.len());
    let function_name = work[..func_end].trim().to_string();

    // Check for source location: "at FILE:LINE"
    let source_location = if let Some(at_pos) = work.find(" at ") {
        let after_at = &work[at_pos + 4..];
        parse_source_location(after_at)
    } else {
        None
    };

    // Check for library: "from LIB"
    let from_lib = if let Some(from_pos) = work.find(" from ") {
        let after_from = &work[from_pos + 6..];
        // Library path extends to end of line (or next whitespace token)
        let lib = after_from.trim().to_string();
        if lib.is_empty() {
            None
        } else {
            Some(lib)
        }
    } else {
        None
    };

    (function_name, source_location, from_lib)
}

/// Parse "FILE:LINE" from a source location string.
fn parse_source_location(s: &str) -> Option<(String, u32)> {
    // Find the last colon (handles paths with colons, e.g., Windows paths)
    let colon_pos = s.rfind(':')?;
    let file = s[..colon_pos].trim();
    let line_str = s[colon_pos + 1..].trim();

    // Line number: take only digits (ignore trailing garbage)
    let line_end = line_str
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(line_str.len());
    if line_end == 0 {
        return None;
    }
    let line: u32 = line_str[..line_end].parse().ok()?;

    Some((file.to_string(), line))
}

/// Detect which thread is the crash thread.
///
/// Priority order:
/// 1. Thread with `<signal handler called>` frame
/// 2. Thread whose top frame is a signal-related function
/// 3. Thread whose stack contains panic-related functions
/// 4. If multiple candidates, prefer signal handler with lowest frame number
fn detect_crash_thread(threads: &[GdbThread]) -> Option<usize> {
    // Priority 1: <signal handler called>
    let mut signal_handler_candidates: Vec<(usize, u32)> = Vec::new();
    for (i, thread) in threads.iter().enumerate() {
        for frame in &thread.frames {
            if frame.is_signal_handler {
                signal_handler_candidates.push((i, frame.frame_num));
            }
        }
    }
    if !signal_handler_candidates.is_empty() {
        // Prefer lowest frame number (closest to crash point)
        signal_handler_candidates.sort_by_key(|&(_, fnum)| fnum);
        return Some(signal_handler_candidates[0].0);
    }

    // Priority 2: top frame is signal-related
    for (i, thread) in threads.iter().enumerate() {
        if let Some(top) = thread.frames.first() {
            let leaf = crate::function_map::extract_leaf_name(&top.function_name);
            if SIGNAL_FUNCTIONS.iter().any(|&s| leaf == s || top.function_name.contains(s)) {
                return Some(i);
            }
        }
    }

    // Priority 3: stack contains panic-related functions
    for (i, thread) in threads.iter().enumerate() {
        for frame in &thread.frames {
            if PANIC_FUNCTIONS
                .iter()
                .any(|&p| frame.function_name.contains(p))
            {
                return Some(i);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_thread_header_full() {
        let line = r#"Thread 15 (Thread 0x7f4c8a1ff640 (LWP 12345) "mettatron"):"#;
        let thread = try_parse_thread_header(line).expect("should parse thread header");
        assert_eq!(thread.gdb_thread_num, 15);
        assert_eq!(thread.lwp, Some(12345));
        assert_eq!(thread.thread_name.as_deref(), Some("mettatron"));
    }

    #[test]
    fn test_parse_thread_header_no_name() {
        let line = "Thread 3 (Thread 0x7f4c8a1ff640 (LWP 99)):";
        let thread = try_parse_thread_header(line).expect("should parse");
        assert_eq!(thread.gdb_thread_num, 3);
        assert_eq!(thread.lwp, Some(99));
        assert_eq!(thread.thread_name, None);
    }

    #[test]
    fn test_parse_frame_with_source() {
        let line = "#1  0x000055a1b2c3d4e5 in mettatron::backend::eval::eval_trampoline_generic () at src/backend/eval/trampoline/generic_trampoline.rs:1234";
        let frame = try_parse_frame(line).expect("should parse frame");
        assert_eq!(frame.frame_num, 1);
        assert!(frame.function_name.contains("eval_trampoline_generic"));
        let (file, line_num) = frame.source_location.expect("should have source");
        assert!(file.contains("generic_trampoline.rs"));
        assert_eq!(line_num, 1234);
        assert!(!frame.is_signal_handler);
    }

    #[test]
    fn test_parse_frame_with_library() {
        let line =
            "#0  0x00007f4c8a1b2c3d in __GI___nanosleep () from /usr/lib/libc.so.6";
        let frame = try_parse_frame(line).expect("should parse frame");
        assert_eq!(frame.frame_num, 0);
        assert!(frame.function_name.contains("nanosleep"));
        assert_eq!(frame.from_lib.as_deref(), Some("/usr/lib/libc.so.6"));
        assert!(frame.source_location.is_none());
    }

    #[test]
    fn test_parse_frame_signal_handler() {
        let line = "#2  <signal handler called>";
        let frame = try_parse_frame(line).expect("should parse signal handler");
        assert_eq!(frame.frame_num, 2);
        assert!(frame.is_signal_handler);
        assert_eq!(frame.function_name, "<signal handler called>");
    }

    #[test]
    fn test_crash_thread_detection_signal_handler() {
        let threads = vec![
            GdbThread {
                gdb_thread_num: 1,
                lwp: Some(100),
                thread_name: Some("worker-0".to_string()),
                frames: vec![GdbFrame {
                    frame_num: 0,
                    function_name: "__GI___nanosleep".to_string(),
                    source_location: None,
                    from_lib: Some("/usr/lib/libc.so.6".to_string()),
                    is_signal_handler: false,
                    category: TraceCategory::Other,
                }],
                is_crash_thread: false,
            },
            GdbThread {
                gdb_thread_num: 2,
                lwp: Some(101),
                thread_name: Some("crash-thread".to_string()),
                frames: vec![
                    GdbFrame {
                        frame_num: 0,
                        function_name: "some_func".to_string(),
                        source_location: None,
                        from_lib: None,
                        is_signal_handler: false,
                        category: TraceCategory::Other,
                    },
                    GdbFrame {
                        frame_num: 1,
                        function_name: "<signal handler called>".to_string(),
                        source_location: None,
                        from_lib: None,
                        is_signal_handler: true,
                        category: TraceCategory::Other,
                    },
                ],
                is_crash_thread: false,
            },
        ];

        let idx = detect_crash_thread(&threads);
        assert_eq!(idx, Some(1), "should detect thread with signal handler");
    }

    #[test]
    fn test_crash_thread_detection_panic() {
        let threads = vec![GdbThread {
            gdb_thread_num: 1,
            lwp: Some(100),
            thread_name: Some("main".to_string()),
            frames: vec![
                GdbFrame {
                    frame_num: 0,
                    function_name: "core::fmt::write".to_string(),
                    source_location: None,
                    from_lib: None,
                    is_signal_handler: false,
                    category: TraceCategory::Other,
                },
                GdbFrame {
                    frame_num: 1,
                    function_name: "rust_begin_unwind".to_string(),
                    source_location: None,
                    from_lib: None,
                    is_signal_handler: false,
                    category: TraceCategory::Other,
                },
            ],
            is_crash_thread: false,
        }];

        let idx = detect_crash_thread(&threads);
        assert_eq!(idx, Some(0), "should detect thread with panic unwind");
    }

    #[test]
    fn test_full_parse_multi_thread() {
        let input = r#"
Thread 15 (Thread 0x7f4c8a1ff640 (LWP 12345) "mettatron"):
#0  0x00007f4c8a1b2c3d in __GI___nanosleep () from /usr/lib/libc.so.6
#1  0x000055a1b2c3d4e5 in mettatron::backend::eval::eval_trampoline_generic () at src/backend/eval/trampoline/generic_trampoline.rs:1234

Thread 14 (Thread 0x7f4c8a0fe640 (LWP 12346) "mettatron-wk-0"):
#0  0x000055a1b2c3d4e5 in mettatron::slab::SlabAllocator::alloc_value () at src/slab/mod.rs:123
#1  <signal handler called>
#2  0x000055a1b2000000 in mettatron::backend::eval::eval_inner () at src/backend/eval/mod.rs:456
"#;

        let bt = parse_gdb_backtrace(input).expect("should parse");
        assert_eq!(bt.threads.len(), 2);

        // Thread 15
        assert_eq!(bt.threads[0].gdb_thread_num, 15);
        assert_eq!(bt.threads[0].lwp, Some(12345));
        assert_eq!(bt.threads[0].thread_name.as_deref(), Some("mettatron"));
        assert_eq!(bt.threads[0].frames.len(), 2);

        // Thread 14
        assert_eq!(bt.threads[1].gdb_thread_num, 14);
        assert_eq!(bt.threads[1].lwp, Some(12346));
        assert_eq!(bt.threads[1].thread_name.as_deref(), Some("mettatron-wk-0"));
        assert_eq!(bt.threads[1].frames.len(), 3);

        // Crash thread detection: thread 14 has signal handler
        assert_eq!(bt.crash_thread_idx, Some(1));
        assert!(bt.threads[1].is_crash_thread);
        assert!(!bt.threads[0].is_crash_thread);
    }

    #[test]
    fn test_parse_frame_classification() {
        let line = "#3  0x000055a1 in mettatron::backend::eval::trampoline::eval_trampoline_generic () at src/backend/eval/trampoline/generic_trampoline.rs:100";
        let frame = try_parse_frame(line).expect("should parse");
        assert_eq!(frame.category, TraceCategory::EvalCore);
    }

    #[test]
    fn test_parse_frame_inlined() {
        // Some GDB outputs omit the address for inlined frames
        let line = "#5  mettatron::backend::eval::bindings::apply_bindings_generic () at src/backend/eval/bindings.rs:42";
        let frame = try_parse_frame(line).expect("should parse inlined frame");
        assert_eq!(frame.frame_num, 5);
        assert!(frame.function_name.contains("apply_bindings_generic"));
        assert_eq!(frame.category, TraceCategory::PatternBinding);
        let (file, line_num) = frame.source_location.expect("should have source");
        assert!(file.contains("bindings.rs"));
        assert_eq!(line_num, 42);
    }
}
