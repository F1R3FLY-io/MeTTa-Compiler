// gdb_correlate.rs — Cross-correlate GDB coredump backtrace with MeTTa evaluation trace.
//
// Bridges native crash analysis (GDB backtraces) with MeTTa evaluation context
// (.mtrace files) by classifying both into a shared category taxonomy. Since
// trace thread IDs are internal sequential counters (not OS LWPs), direct
// per-thread correlation is impossible; instead, we use category-level semantic
// matching to find trace events that were likely executing at crash time.

use std::collections::{HashMap, VecDeque};

use trace_format::TraceEvent;

use crate::function_map::{
    classify_trace_event, TraceCategory, ALL_CATEGORIES,
};
use crate::gdb_parser::{parse_gdb_backtrace, GdbBacktrace};
use crate::reader::TraceReader;
use crate::util::{extract_operator_name, format_duration_ns};

// ── Ring Buffer ──────────────────────────────────────────────────────────────

/// Fixed-capacity ring buffer that retains the last `capacity` events per thread.
struct TailRingBuffer {
    events: VecDeque<TraceEvent>,
    capacity: usize,
    total_count: u64,
}

impl TailRingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity,
            total_count: 0,
        }
    }

    fn push(&mut self, event: TraceEvent) {
        self.total_count += 1;
        if self.events.len() == self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }
}

// ── Per-thread summary ───────────────────────────────────────────────────────

struct ThreadSummary {
    event_count: u64,
    last_timestamp: u64,
    last_expression: String,
    last_category: TraceCategory,
    tail: TailRingBuffer,
}

impl ThreadSummary {
    fn new(tail_capacity: usize) -> Self {
        Self {
            event_count: 0,
            last_timestamp: 0,
            last_expression: String::new(),
            last_category: TraceCategory::Other,
            tail: TailRingBuffer::new(tail_capacity),
        }
    }
}

// ── Head symbol frequency tracker ────────────────────────────────────────────

struct ExprFrequency {
    count: u64,
    last_timestamp: u64,
    total_depth: u64,
}

// ── Investigation hints ──────────────────────────────────────────────────────

fn investigation_hint(category: TraceCategory) -> &'static str {
    match category {
        TraceCategory::Allocation => {
            "Crash in allocation subsystem. Check for:\n\
             - Use-after-free (slab page reuse with stale pointer)\n\
             - Slab exhaustion (all pages full, no free list entries)\n\
             - Double-free (page live_count underflow)\n\
             - Concurrent alloc/free race (check atomic ordering)"
        }
        TraceCategory::GarbageCollection => {
            "Crash in GC subsystem. Check for:\n\
             - Unregistered roots (object moved/freed while still reachable)\n\
             - Safepoint missed (mutation during mark phase)\n\
             - Epoch-based filtering error (live object treated as garbage)\n\
             - Concurrent GC vs allocator race"
        }
        TraceCategory::EvalCore => {
            "Crash in evaluation core. Check for:\n\
             - Stack overflow from unbounded recursion\n\
             - Invalid MettaValue pointer (dangling inner ref)\n\
             - Trampoline continuation corruption\n\
             - Work stack bounds violation"
        }
        TraceCategory::RuleMatching => {
            "Crash in rule matching. Check for:\n\
             - RuleIndex corruption (stale entries after remove-atom)\n\
             - Bloom filter false-positive leading to invalid access\n\
             - Pattern match on freed MettaValue"
        }
        TraceCategory::PatternBinding => {
            "Crash in pattern binding/unification. Check for:\n\
             - Binding map referencing freed values\n\
             - Infinite unification loop (cyclic bindings)\n\
             - apply_bindings on corrupted environment"
        }
        TraceCategory::Nondeterminism => {
            "Crash in nondeterministic branching. Check for:\n\
             - Work-stealing race (task consumed twice)\n\
             - Branch budget underflow\n\
             - Parallel branch result collection corruption"
        }
        TraceCategory::BytecodeVM => {
            "Crash in bytecode VM. Check for:\n\
             - Invalid opcode (corrupted bytecode buffer)\n\
             - VM stack underflow/overflow\n\
             - Bytecode-to-value conversion on freed pointer"
        }
        TraceCategory::JitCompilation => {
            "Crash in JIT compilation/execution. Check for:\n\
             - NaN-boxing tag corruption (TAG_PTR with invalid pointer)\n\
             - JIT native function stack-use-after-return\n\
             - TypeSignatureRegistry lifetime issue"
        }
        TraceCategory::MorkOps => {
            "Crash in MORK/PathMap subsystem. Check for:\n\
             - SharedMapping concurrent modification\n\
             - Trie node corruption\n\
             - Symbol cache ABA (pointer reuse with stale hash)"
        }
        TraceCategory::SpaceOps => {
            "Crash in space operations. Check for:\n\
             - AtomSpace concurrent add/remove race\n\
             - match_space query on partially-modified environment\n\
             - SpaceHandle vs environment PathMap routing mismatch"
        }
        TraceCategory::TypeSystem => {
            "Crash in type system. Check for:\n\
             - Type registry concurrent access\n\
             - Infinite type inference loop\n\
             - Arrow type deconstruction on non-arrow value"
        }
        _ => {
            "No specific hints for this crash category.\n\
             General checks: memory corruption, thread safety, null/dangling pointers."
        }
    }
}

/// Adjacent categories for broadened matching when exact match yields no results.
fn adjacent_categories(cat: TraceCategory) -> Vec<TraceCategory> {
    match cat {
        TraceCategory::Allocation => vec![TraceCategory::GarbageCollection, TraceCategory::EvalCore],
        TraceCategory::GarbageCollection => vec![TraceCategory::Allocation, TraceCategory::EvalCore],
        TraceCategory::EvalCore => vec![
            TraceCategory::RuleMatching,
            TraceCategory::PatternBinding,
            TraceCategory::ControlFlow,
        ],
        TraceCategory::RuleMatching => vec![TraceCategory::PatternBinding, TraceCategory::EvalCore],
        TraceCategory::PatternBinding => vec![TraceCategory::RuleMatching, TraceCategory::EvalCore],
        TraceCategory::Nondeterminism => vec![TraceCategory::EvalCore, TraceCategory::RuleMatching],
        TraceCategory::BytecodeVM => vec![TraceCategory::EvalCore, TraceCategory::JitCompilation],
        TraceCategory::JitCompilation => vec![TraceCategory::EvalCore, TraceCategory::BytecodeVM],
        TraceCategory::MorkOps => vec![TraceCategory::SpaceOps, TraceCategory::RuleMatching],
        TraceCategory::SpaceOps => vec![TraceCategory::MorkOps, TraceCategory::EvalCore],
        _ => vec![TraceCategory::EvalCore],
    }
}

// ── Main entry point ─────────────────────────────────────────────────────────

pub fn run(
    trace_file: &str,
    gdb_bt_file: &str,
    tail_n: usize,
    top_n: usize,
    json: bool,
) -> Result<(), String> {
    // 1. Parse GDB backtrace
    let bt_text = std::fs::read_to_string(gdb_bt_file)
        .map_err(|e| format!("Failed to read GDB backtrace file: {e}"))?;
    let backtrace = parse_gdb_backtrace(&bt_text)?;

    // 2. Identify crash category
    let crash_category = identify_crash_category(&backtrace);

    // 3. Read trace file (potentially truncated)
    let reader = TraceReader::open(trace_file)?;

    // 4. Single-pass tail extraction
    let mut thread_summaries: HashMap<u32, ThreadSummary> = HashMap::new();
    let mut total_events: u64 = 0;
    let mut category_counts: HashMap<TraceCategory, u64> = HashMap::new();

    for event in reader.events() {
        total_events += 1;

        let cat = classify_trace_event(&event.kind);
        *category_counts.entry(cat).or_insert(0) += 1;

        let head = extract_operator_name(&event.input, &event.kind);
        let thread_id = event.thread_id;

        let summary = thread_summaries
            .entry(thread_id)
            .or_insert_with(|| ThreadSummary::new(tail_n));
        summary.event_count += 1;
        summary.last_timestamp = event.timestamp_ns;
        summary.last_expression = head;
        summary.last_category = cat;

        if tail_n > 0 {
            summary.tail.push(event);
        }
    }

    // 5. Post-scan analysis
    let mut category_matched_events: Vec<(u32, &TraceEvent)> = Vec::new();
    let match_categories = if let Some(crash_cat) = crash_category {
        let mut cats = vec![crash_cat];
        // Collect exact matches first
        let mut exact_count = 0;
        for summary in thread_summaries.values() {
            for event in &summary.tail.events {
                if classify_trace_event(&event.kind) == crash_cat {
                    exact_count += 1;
                }
            }
        }
        // If no exact matches, broaden to adjacent categories
        if exact_count == 0 {
            cats.extend(adjacent_categories(crash_cat));
        }
        cats
    } else {
        vec![]
    };

    for summary in thread_summaries.values() {
        for event in &summary.tail.events {
            let cat = classify_trace_event(&event.kind);
            if match_categories.contains(&cat) {
                category_matched_events.push((event.thread_id, event));
            }
        }
    }

    // Sort matched events by timestamp (most recent first)
    category_matched_events.sort_by(|a, b| b.1.timestamp_ns.cmp(&a.1.timestamp_ns));

    // Expression frequency in tail events
    let mut expr_freq: HashMap<String, ExprFrequency> = HashMap::new();
    for summary in thread_summaries.values() {
        for event in &summary.tail.events {
            let head = extract_operator_name(&event.input, &event.kind);
            let entry = expr_freq.entry(head).or_insert(ExprFrequency {
                count: 0,
                last_timestamp: 0,
                total_depth: 0,
            });
            entry.count += 1;
            if event.timestamp_ns > entry.last_timestamp {
                entry.last_timestamp = event.timestamp_ns;
            }
            entry.total_depth += event.depth as u64;
        }
    }

    // 6. Output
    if json {
        print_json(
            &backtrace,
            crash_category,
            &reader,
            total_events,
            &thread_summaries,
            &category_matched_events,
            &expr_freq,
            top_n,
        );
    } else {
        print_text(
            &backtrace,
            crash_category,
            &reader,
            total_events,
            &thread_summaries,
            &category_matched_events,
            &expr_freq,
            &category_counts,
            top_n,
        );
    }

    Ok(())
}

/// Identify the crash category from the GDB backtrace.
///
/// Finds the first non-signal-handler, non-libc frame on the crash thread
/// and returns its category.
fn identify_crash_category(bt: &GdbBacktrace) -> Option<TraceCategory> {
    let crash_idx = bt.crash_thread_idx?;
    let crash_thread = &bt.threads[crash_idx];

    for frame in &crash_thread.frames {
        if frame.is_signal_handler {
            continue;
        }
        // Skip libc/kernel frames
        if let Some(ref lib) = frame.from_lib {
            if lib.contains("libc") || lib.contains("libpthread") || lib.contains("linux-vdso") {
                continue;
            }
        }
        if frame.function_name.starts_with("__GI_")
            || frame.function_name.starts_with("__libc_")
            || frame.function_name == "__restore_rt"
        {
            continue;
        }

        return Some(frame.category);
    }

    None
}

// ── Text output ──────────────────────────────────────────────────────────────

fn print_text(
    bt: &GdbBacktrace,
    crash_category: Option<TraceCategory>,
    reader: &TraceReader,
    total_events: u64,
    thread_summaries: &HashMap<u32, ThreadSummary>,
    category_matched: &[(u32, &TraceEvent)],
    expr_freq: &HashMap<String, ExprFrequency>,
    category_counts: &HashMap<TraceCategory, u64>,
    top_n: usize,
) {
    println!("=== GDB Coredump Cross-Correlation Report ===");
    println!();

    // Section 1: Crash Summary
    print_crash_summary(bt, crash_category);

    // Section 2: Truncation Warning
    if reader.truncated {
        println!();
        println!("!!! WARNING: Trace file appears CRASH-TRUNCATED (no valid footer) !!!");
        println!("    Some events near the crash point may be lost or incomplete.");
        println!("    The last event(s) in the trace may not represent the actual crash moment.");
    }

    // Section 3: Trace Overview
    println!();
    println!("--- Trace Overview ---");
    println!();
    println!("Total events: {}", total_events);
    println!("Trace threads: {}", thread_summaries.len());
    println!(
        "Trace file: {}",
        if reader.truncated {
            "TRUNCATED (crash)"
        } else {
            "complete"
        }
    );

    // Category distribution
    println!();
    println!("Category distribution (all events):");
    let mut cat_list: Vec<_> = ALL_CATEGORIES
        .iter()
        .filter_map(|&cat| {
            let count = *category_counts.get(&cat).unwrap_or(&0);
            if count > 0 {
                Some((cat, count))
            } else {
                None
            }
        })
        .collect();
    cat_list.sort_by(|a, b| b.1.cmp(&a.1));
    for (cat, count) in &cat_list {
        let pct = if total_events > 0 {
            *count as f64 / total_events as f64 * 100.0
        } else {
            0.0
        };
        println!("  {:<20} {:>8} ({:.1}%)", format!("{}", cat), count, pct);
    }

    // Section 4: Per-thread tail summary
    println!();
    println!("--- Per-Thread Tail Summary ---");
    println!();
    println!(
        "{:<10} {:>10} {:>16} {:<20} {:<16}",
        "Thread", "Events", "Last Timestamp", "Last Expression", "Last Category"
    );
    println!("{}", "-".repeat(76));

    let mut thread_ids: Vec<u32> = thread_summaries.keys().copied().collect();
    thread_ids.sort();
    for tid in &thread_ids {
        let summary = &thread_summaries[tid];
        let expr_display = if summary.last_expression.len() > 20 {
            format!("{}...", &summary.last_expression[..17])
        } else {
            summary.last_expression.clone()
        };
        println!(
            "{:<10} {:>10} {:>16} {:<20} {:<16}",
            tid,
            summary.event_count,
            format_duration_ns(summary.last_timestamp),
            expr_display,
            format!("{}", summary.last_category),
        );
    }

    // Section 5: Category-Matched Events
    if !category_matched.is_empty() {
        println!();
        let cat_label = crash_category
            .map(|c| format!("{}", c))
            .unwrap_or_else(|| "Unknown".to_string());
        println!(
            "--- Category-Matched Trace Events (crash category: {}) ---",
            cat_label
        );
        println!();
        println!(
            "{:<16} {:<8} {:<6} {:<16} {:<30}",
            "Timestamp", "Thread", "Depth", "Category", "Expression"
        );
        println!("{}", "-".repeat(80));

        for (_, event) in category_matched.iter().take(top_n) {
            let cat = classify_trace_event(&event.kind);
            let head = extract_operator_name(&event.input, &event.kind);
            let input_str = format!("{}", event.input);
            let expr_display = if input_str.len() > 30 {
                format!("{}...", &input_str[..27])
            } else {
                input_str
            };
            println!(
                "{:<16} {:<8} {:<6} {:<16} {:<30}",
                format_duration_ns(event.timestamp_ns),
                event.thread_id,
                event.depth,
                format!("{}", cat),
                expr_display,
            );
            // Show event kind detail
            let kind_detail = format!("{:?}", event.kind);
            if kind_detail.len() > 76 {
                println!("  -> {}...", &kind_detail[..73]);
            } else {
                println!("  -> {}", kind_detail);
            }
            let _ = head; // used above via input_str
        }
    } else if crash_category.is_some() {
        println!();
        println!(
            "--- No trace events matched crash category {} ---",
            crash_category.map(|c| format!("{}", c)).unwrap_or_default()
        );
        println!("    The crash may have occurred in a code path not instrumented by tracing.");
    }

    // Section 6: Recent Expression Summary
    if !expr_freq.is_empty() {
        println!();
        println!("--- Recent Expression Summary (tail events) ---");
        println!();
        println!(
            "{:<30} {:>8} {:>16} {:>10}",
            "Head Symbol", "Count", "Last Seen", "Avg Depth"
        );
        println!("{}", "-".repeat(68));

        let mut freq_list: Vec<_> = expr_freq.iter().collect();
        freq_list.sort_by(|a, b| b.1.count.cmp(&a.1.count));

        for (head, freq) in freq_list.iter().take(top_n) {
            let avg_depth = if freq.count > 0 {
                freq.total_depth as f64 / freq.count as f64
            } else {
                0.0
            };
            let head_display = if head.len() > 30 {
                format!("{}...", &head[..27])
            } else {
                (*head).clone()
            };
            println!(
                "{:<30} {:>8} {:>16} {:>10.1}",
                head_display,
                freq.count,
                format_duration_ns(freq.last_timestamp),
                avg_depth,
            );
        }
    }

    // Section 7: Investigation Hints
    if let Some(crash_cat) = crash_category {
        println!();
        println!("--- Investigation Hints ---");
        println!();
        println!("{}", investigation_hint(crash_cat));
    }
}

fn print_crash_summary(bt: &GdbBacktrace, crash_category: Option<TraceCategory>) {
    println!("--- Crash Summary ---");
    println!();

    if let Some(idx) = bt.crash_thread_idx {
        let thread = &bt.threads[idx];
        println!(
            "Crash thread: GDB Thread {} (LWP {}{})",
            thread.gdb_thread_num,
            thread
                .lwp
                .map(|l| l.to_string())
                .unwrap_or_else(|| "?".to_string()),
            thread
                .thread_name
                .as_ref()
                .map(|n| format!(", \"{}\"", n))
                .unwrap_or_default(),
        );

        // Find crash frame (first non-signal-handler, non-libc frame)
        for frame in &thread.frames {
            if frame.is_signal_handler {
                continue;
            }
            if let Some(ref lib) = frame.from_lib {
                if lib.contains("libc") || lib.contains("libpthread") {
                    continue;
                }
            }
            if frame.function_name.starts_with("__GI_")
                || frame.function_name.starts_with("__libc_")
                || frame.function_name == "__restore_rt"
            {
                continue;
            }

            println!("Crash frame: #{} {}", frame.frame_num, frame.function_name);
            if let Some((ref file, line)) = frame.source_location {
                println!("  at {}:{}", file, line);
            }
            println!("Crash category: {}", frame.category);
            break;
        }

        // Print full stack with per-frame category labels
        println!();
        println!("Full crash thread stack:");
        for frame in &thread.frames {
            let cat_label = if frame.is_signal_handler {
                "---SIGNAL---".to_string()
            } else {
                format!("{}", frame.category)
            };

            let loc = if let Some((ref file, line)) = frame.source_location {
                format!(" at {}:{}", file, line)
            } else if let Some(ref lib) = frame.from_lib {
                format!(" from {}", lib)
            } else {
                String::new()
            };

            let func_display = if frame.function_name.len() > 60 {
                format!("{}...", &frame.function_name[..57])
            } else {
                frame.function_name.clone()
            };

            println!(
                "  #{:<3} [{:<16}] {}{}",
                frame.frame_num, cat_label, func_display, loc
            );
        }
    } else {
        println!("No crash thread identified in GDB backtrace.");
        println!("Total threads: {}", bt.threads.len());
    }

    if let Some(cat) = crash_category {
        println!();
        println!("Crash category: {}", cat);
    }
}

// ── JSON output ──────────────────────────────────────────────────────────────

fn print_json(
    bt: &GdbBacktrace,
    crash_category: Option<TraceCategory>,
    reader: &TraceReader,
    total_events: u64,
    thread_summaries: &HashMap<u32, ThreadSummary>,
    category_matched: &[(u32, &TraceEvent)],
    expr_freq: &HashMap<String, ExprFrequency>,
    top_n: usize,
) {
    println!("{{");

    // Truncation status
    println!("  \"truncated\": {},", reader.truncated);

    // Crash summary
    println!("  \"crash_summary\": {{");
    if let Some(idx) = bt.crash_thread_idx {
        let thread = &bt.threads[idx];
        println!("    \"gdb_thread_num\": {},", thread.gdb_thread_num);
        if let Some(lwp) = thread.lwp {
            println!("    \"lwp\": {},", lwp);
        }
        if let Some(ref name) = thread.thread_name {
            println!("    \"thread_name\": {:?},", name);
        }
        if let Some(cat) = crash_category {
            println!("    \"crash_category\": \"{}\",", cat);
        }

        // Crash frame
        for frame in &thread.frames {
            if frame.is_signal_handler
                || frame.function_name.starts_with("__GI_")
                || frame.function_name.starts_with("__libc_")
                || frame.function_name == "__restore_rt"
            {
                continue;
            }
            if let Some(ref lib) = frame.from_lib {
                if lib.contains("libc") || lib.contains("libpthread") {
                    continue;
                }
            }
            println!("    \"crash_frame\": {{");
            println!("      \"frame_num\": {},", frame.frame_num);
            println!("      \"function\": {:?},", frame.function_name);
            if let Some((ref file, line)) = frame.source_location {
                println!("      \"file\": {:?},", file);
                println!("      \"line\": {},", line);
            }
            println!("      \"category\": \"{}\"", frame.category);
            println!("    }},");
            break;
        }
    }
    println!("    \"total_gdb_threads\": {}", bt.threads.len());
    println!("  }},");

    // GDB stack (crash thread only)
    println!("  \"gdb_stack\": [");
    if let Some(idx) = bt.crash_thread_idx {
        let frames = &bt.threads[idx].frames;
        for (i, frame) in frames.iter().enumerate() {
            let comma = if i + 1 < frames.len() { "," } else { "" };
            print!("    {{\"frame\": {}, \"function\": {:?}", frame.frame_num, frame.function_name);
            if let Some((ref file, line)) = frame.source_location {
                print!(", \"file\": {:?}, \"line\": {}", file, line);
            }
            if let Some(ref lib) = frame.from_lib {
                print!(", \"from_lib\": {:?}", lib);
            }
            print!(
                ", \"signal_handler\": {}, \"category\": \"{}\"",
                frame.is_signal_handler, frame.category
            );
            println!("}}{}", comma);
        }
    }
    println!("  ],");

    // Trace summary
    println!("  \"trace_summary\": {{");
    println!("    \"total_events\": {},", total_events);
    println!("    \"trace_threads\": {},", thread_summaries.len());

    // Per-thread info
    println!("    \"threads\": [");
    let mut thread_ids: Vec<u32> = thread_summaries.keys().copied().collect();
    thread_ids.sort();
    for (i, tid) in thread_ids.iter().enumerate() {
        let summary = &thread_summaries[tid];
        let comma = if i + 1 < thread_ids.len() { "," } else { "" };
        println!(
            "      {{\"thread_id\": {}, \"event_count\": {}, \"last_timestamp_ns\": {}, \"last_expression\": {:?}, \"last_category\": \"{}\"}}{}",
            tid, summary.event_count, summary.last_timestamp, summary.last_expression, summary.last_category, comma
        );
    }
    println!("    ]");
    println!("  }},");

    // Category-matched events
    println!("  \"category_matched_events\": [");
    let matched_slice: Vec<_> = category_matched.iter().take(top_n).collect();
    for (i, (_, event)) in matched_slice.iter().enumerate() {
        let cat = classify_trace_event(&event.kind);
        let comma = if i + 1 < matched_slice.len() { "," } else { "" };
        println!(
            "    {{\"timestamp_ns\": {}, \"thread_id\": {}, \"depth\": {}, \"category\": \"{}\", \"expression\": {:?}}}{}",
            event.timestamp_ns, event.thread_id, event.depth, cat,
            format!("{}", event.input), comma
        );
    }
    println!("  ],");

    // Expression summary
    println!("  \"expression_summary\": [");
    let mut freq_list: Vec<_> = expr_freq.iter().collect();
    freq_list.sort_by(|a, b| b.1.count.cmp(&a.1.count));
    let freq_slice: Vec<_> = freq_list.iter().take(top_n).collect();
    for (i, (head, freq)) in freq_slice.iter().enumerate() {
        let avg_depth = if freq.count > 0 {
            freq.total_depth as f64 / freq.count as f64
        } else {
            0.0
        };
        let comma = if i + 1 < freq_slice.len() { "," } else { "" };
        println!(
            "    {{\"head_symbol\": {:?}, \"count\": {}, \"last_timestamp_ns\": {}, \"avg_depth\": {:.1}}}{}",
            head, freq.count, freq.last_timestamp, avg_depth, comma
        );
    }
    println!("  ]");

    println!("}}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_event(ts: u64, input: trace_format::TraceValue) -> TraceEvent {
        TraceEvent {
            seq: ts,
            timestamp_ns: ts,
            thread_id: 0,
            tier: trace_format::TraceTier::TreeWalker,
            depth: 0,
            input,
            outputs: vec![],
            expr_span: None,
            kind: trace_format::TraceEventKind::EvalStart,
            duration_ns: None,
            span_id: None,
        }
    }

    #[test]
    fn test_ring_buffer_capacity() {
        let mut rb = TailRingBuffer::new(3);
        for i in 0..10u64 {
            rb.push(make_test_event(i, trace_format::TraceValue::Long(i as i64)));
        }
        assert_eq!(rb.total_count, 10);
        assert_eq!(rb.events.len(), 3);
        // Should retain last 3 events (timestamps 7, 8, 9)
        assert_eq!(rb.events[0].timestamp_ns, 7);
        assert_eq!(rb.events[1].timestamp_ns, 8);
        assert_eq!(rb.events[2].timestamp_ns, 9);
    }

    #[test]
    fn test_ring_buffer_under_capacity() {
        let mut rb = TailRingBuffer::new(10);
        rb.push(make_test_event(42, trace_format::TraceValue::Atom("test".to_string())));
        assert_eq!(rb.total_count, 1);
        assert_eq!(rb.events.len(), 1);
        assert_eq!(rb.events[0].timestamp_ns, 42);
    }

    #[test]
    fn test_investigation_hint_allocation() {
        let hint = investigation_hint(TraceCategory::Allocation);
        assert!(hint.contains("Use-after-free"));
        assert!(hint.contains("slab"));
    }

    #[test]
    fn test_adjacent_categories() {
        let adj = adjacent_categories(TraceCategory::Allocation);
        assert!(adj.contains(&TraceCategory::GarbageCollection));
        assert!(adj.contains(&TraceCategory::EvalCore));
    }

    #[test]
    fn test_identify_crash_category() {
        use crate::gdb_parser::{GdbFrame, GdbThread};

        let bt = GdbBacktrace {
            threads: vec![GdbThread {
                gdb_thread_num: 1,
                lwp: Some(100),
                thread_name: Some("main".to_string()),
                frames: vec![
                    GdbFrame {
                        frame_num: 0,
                        function_name: "<signal handler called>".to_string(),
                        source_location: None,
                        from_lib: None,
                        is_signal_handler: true,
                        category: TraceCategory::Other,
                    },
                    GdbFrame {
                        frame_num: 1,
                        function_name: "__GI___nanosleep".to_string(),
                        source_location: None,
                        from_lib: Some("/usr/lib/libc.so.6".to_string()),
                        is_signal_handler: false,
                        category: TraceCategory::Other,
                    },
                    GdbFrame {
                        frame_num: 2,
                        function_name: "mettatron::slab::SlabAllocator::alloc_value".to_string(),
                        source_location: Some(("src/slab/mod.rs".to_string(), 123)),
                        from_lib: None,
                        is_signal_handler: false,
                        category: TraceCategory::Allocation,
                    },
                ],
                is_crash_thread: true,
            }],
            crash_thread_idx: Some(0),
        };

        let cat = identify_crash_category(&bt);
        assert_eq!(cat, Some(TraceCategory::Allocation));
    }
}
