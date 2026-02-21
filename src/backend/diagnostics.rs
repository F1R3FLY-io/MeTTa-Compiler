//! Signal-triggered diagnostic dump for livelock debugging.
//!
//! Installs signal handlers for:
//! - **SIGTERM** → dump diagnostics, then re-raise (exit code 143)
//! - **SIGUSR1** → dump diagnostics, continue running (repeatable)
//!
//! Signal handlers only set atomic flags (async-signal-safe via `signal-hook`).
//! A dedicated daemon thread polls the flags every 50ms and performs all I/O.
//!
//! Thread enumeration uses `/proc/self/task/` on Linux to read thread names,
//! states, and kernel wait channels (wchan). The wchan field is the key
//! diagnostic for livelocks — it shows exactly which kernel function each
//! thread is blocked on (e.g., `futex_wait_queue`, `ep_poll`, `hrtimer_nanosleep`).
//!
//! Auto-installed via `global_allocator()` so tests get coverage without
//! explicit setup. Disabled via `METTATRON_NO_SIGNAL_DIAG=1` env var.

use std::sync::Once;

#[cfg(unix)]
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

/// Guard to ensure `install_signal_handlers()` runs at most once.
static INSTALL_ONCE: Once = Once::new();

/// Install SIGTERM and SIGUSR1 diagnostic handlers.
///
/// Idempotent — repeated calls are a single atomic load after first init.
/// Disabled when `METTATRON_NO_SIGNAL_DIAG=1` is set.
///
/// This is automatically called from `global_allocator()`, so every code path
/// (binary, tests, benchmarks) gets coverage without explicit setup.
#[cfg(unix)]
pub fn install_signal_handlers() {
    INSTALL_ONCE.call_once(|| {
        // Check opt-out env var
        if std::env::var("METTATRON_NO_SIGNAL_DIAG")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            return;
        }

        // Create shared Arc<AtomicBool> flags for signal-hook
        let sigterm_flag = Arc::new(AtomicBool::new(false));
        let sigusr1_flag = Arc::new(AtomicBool::new(false));

        // Register signal flags (async-signal-safe: only sets atomics)
        if let Err(e) = signal_hook::flag::register(
            signal_hook::consts::SIGTERM,
            Arc::clone(&sigterm_flag),
        ) {
            eprintln!("[diagnostics] Failed to register SIGTERM handler: {}", e);
            return;
        }
        if let Err(e) = signal_hook::flag::register(
            signal_hook::consts::SIGUSR1,
            Arc::clone(&sigusr1_flag),
        ) {
            eprintln!("[diagnostics] Failed to register SIGUSR1 handler: {}", e);
            return;
        }

        // Spawn daemon watcher thread (won't prevent process exit)
        thread::Builder::new()
            .name("diag-watcher".to_string())
            .spawn(move || signal_watcher_loop(sigterm_flag, sigusr1_flag))
            .expect("Failed to spawn diagnostic watcher thread");
    });
}

/// No-op on non-Unix platforms.
#[cfg(not(unix))]
pub fn install_signal_handlers() {}

/// Watcher thread main loop. Polls signal flags every 50ms.
#[cfg(unix)]
fn signal_watcher_loop(
    sigterm_flag: Arc<AtomicBool>,
    sigusr1_flag: Arc<AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    let poll_interval = Duration::from_millis(50);

    loop {
        thread::sleep(poll_interval);

        if sigterm_flag.swap(false, Ordering::AcqRel) {
            dump_diagnostics("SIGTERM");
            reraise_sigterm();
            // Should not reach here, but break just in case
            break;
        }

        if sigusr1_flag.swap(false, Ordering::AcqRel) {
            dump_diagnostics("SIGUSR1");
            // Continue running — SIGUSR1 is repeatable
        }
    }
}

/// Print the full diagnostic dump to stderr.
#[cfg(unix)]
fn dump_diagnostics(signal_name: &str) {
    let pid = std::process::id();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║  METTATRON DIAGNOSTIC DUMP                                  ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  Signal: {:<10}  PID: {:<10}  Epoch: {:<14} ║", signal_name, pid, timestamp);
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!();

    dump_gc_state();
    dump_evaluator_state();
    dump_thread_info();
    dump_watcher_backtrace();

    eprintln!("═══════════════════════ END DIAGNOSTIC DUMP ═══════════════════");
    eprintln!();
}

/// Dump GC state from existing public APIs.
#[cfg(unix)]
fn dump_gc_state() {
    use crate::backend::models::gc_allocator;

    let alloc = gc_allocator::global_allocator();
    let committed = alloc.committed_bytes();
    let threshold = alloc.gc_threshold();
    let bp_level = gc_allocator::backpressure_level();
    let in_flight = gc_allocator::gc_cycle_in_flight();
    let requested = gc_allocator::is_gc_requested();
    let disabled = gc_allocator::is_gc_disabled();
    let reachable = gc_allocator::gc_reachable_counter();

    let ratio = if threshold > 0 {
        committed as f64 / threshold as f64
    } else {
        0.0
    };

    eprintln!("── GC State ──────────────────────────────────────────────────");
    eprintln!("  committed_bytes:    {:>12} ({:.1} MB)", committed, committed as f64 / (1024.0 * 1024.0));
    eprintln!("  gc_threshold:       {:>12} ({:.1} MB)", threshold, threshold as f64 / (1024.0 * 1024.0));
    eprintln!("  commit/threshold:   {:>12.2}", ratio);
    eprintln!("  backpressure_level: {:>12} (max={})", bp_level, gc_allocator::MAX_BACKPRESSURE);
    eprintln!("  gc_cycle_in_flight: {:>12}", in_flight);
    eprintln!("  gc_requested:       {:>12}", requested);
    eprintln!("  gc_disabled:        {:>12}", disabled);
    eprintln!("  gc_reachable_ctr:   {:>12}", reachable);
    eprintln!();

    // Page statistics
    let ps = alloc.page_stats();
    let dead_slots = ps.total_bumped_slots.saturating_sub(ps.total_live_slots);
    let occupancy = if ps.total_bumped_slots > 0 {
        ps.total_live_slots as f64 / ps.total_bumped_slots as f64 * 100.0
    } else {
        0.0
    };

    eprintln!("── Slab Pages ────────────────────────────────────────────────");
    eprintln!("  value_pages:        {:>12}", ps.value_page_count);
    eprintln!("  slots_per_page:     {:>12}", ps.slots_per_page);
    eprintln!("  total_bumped:       {:>12}", ps.total_bumped_slots);
    eprintln!("  total_live:         {:>12}", ps.total_live_slots);
    eprintln!("  dead (bumped-live): {:>12}", dead_slots);
    eprintln!("  occupancy:          {:>11.1}%", occupancy);
    eprintln!("  value_committed:    {:>12} ({:.1} MB)", ps.value_committed_bytes, ps.value_committed_bytes as f64 / (1024.0 * 1024.0));
    eprintln!("  data_pages:         {:>12}", ps.data_page_count);
    eprintln!("  data_committed:     {:>12} ({:.1} MB)", ps.data_committed_bytes, ps.data_committed_bytes as f64 / (1024.0 * 1024.0));
    eprintln!();

    // Session GC statistics
    let sgc = gc_allocator::session_gc_stats();
    eprintln!("── Session GC ────────────────────────────────────────────────");
    eprintln!("  sessions_released:  {:>12}", sgc.releases_total);
    eprintln!("  values_freed:       {:>12}", sgc.values_freed_total);
    eprintln!("  values_promoted:    {:>12}", sgc.values_promoted_total);
    eprintln!("  values_scanned:     {:>12}", sgc.values_scanned_total);
    eprintln!("  surviving_set_size: {:>12}", sgc.last_surviving_set_size);
    if sgc.releases_total > 0 {
        eprintln!("  avg freed/release:  {:>12.0}", sgc.values_freed_total as f64 / sgc.releases_total as f64);
        eprintln!("  avg promoted/rel:   {:>12.0}", sgc.values_promoted_total as f64 / sgc.releases_total as f64);
    }
    eprintln!();
}

/// Dump evaluator state.
#[cfg(unix)]
fn dump_evaluator_state() {
    use crate::backend::models::gc_allocator;

    let active = gc_allocator::active_evaluator_count();

    eprintln!("── Evaluator State ───────────────────────────────────────────");
    eprintln!("  active_evaluators:  {:>12}", active);
    eprintln!();
}

/// Dump thread information from /proc/self/task/ (Linux-specific).
#[cfg(target_os = "linux")]
fn dump_thread_info() {
    eprintln!("── Thread Table ──────────────────────────────────────────────");
    eprintln!("  {:>7}  {:<24}  {:<12}  {}", "TID", "NAME", "STATE", "WCHAN");
    eprintln!("  {:>7}  {:<24}  {:<12}  {}", "───────", "────────────────────────", "────────────", "──────────────────────");

    let task_dir = match std::fs::read_dir("/proc/self/task") {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("  (failed to read /proc/self/task: {})", e);
            eprintln!();
            return;
        }
    };

    let mut entries: Vec<(u32, String, String, String)> = Vec::new();

    for entry in task_dir.flatten() {
        let tid_str = entry.file_name();
        let tid_str = tid_str.to_string_lossy();
        let tid: u32 = match tid_str.parse() {
            Ok(t) => t,
            Err(_) => continue,
        };

        let name = read_proc_file(&format!("/proc/self/task/{}/comm", tid))
            .unwrap_or_else(|| "???".to_string());

        let state = read_thread_state(tid);
        let wchan = read_proc_file(&format!("/proc/self/task/{}/wchan", tid))
            .unwrap_or_else(|| "???".to_string());

        entries.push((tid, name, state, wchan));
    }

    // Sort by TID for stable output
    entries.sort_by_key(|(tid, _, _, _)| *tid);

    for (tid, name, state, wchan) in &entries {
        eprintln!("  {:>7}  {:<24}  {:<12}  {}", tid, name, state, wchan);
    }

    eprintln!("  ({} threads total)", entries.len());
    eprintln!();
}

/// Read a single-line proc file, trimming whitespace.
#[cfg(target_os = "linux")]
fn read_proc_file(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Extract thread state from /proc/self/task/<tid>/status.
/// Parses the "State:" line (e.g., "S (sleeping)", "R (running)").
#[cfg(target_os = "linux")]
fn read_thread_state(tid: u32) -> String {
    let status_path = format!("/proc/self/task/{}/status", tid);
    let content = match std::fs::read_to_string(&status_path) {
        Ok(c) => c,
        Err(_) => return "???".to_string(),
    };

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("State:") {
            return rest.trim().to_string();
        }
    }

    "???".to_string()
}

/// Stub for non-Linux Unix platforms — just print a note.
#[cfg(all(unix, not(target_os = "linux")))]
fn dump_thread_info() {
    eprintln!("── Thread Table ──────────────────────────────────────────────");
    eprintln!("  (thread enumeration requires Linux /proc/self/task/)");
    eprintln!();
}

/// Capture and print a backtrace from the watcher thread itself.
/// This provides context about the watcher's position in case it's stuck.
#[cfg(unix)]
fn dump_watcher_backtrace() {
    eprintln!("── Watcher Thread Backtrace ───────────────────────────────────");
    let bt = std::backtrace::Backtrace::force_capture();
    eprintln!("{}", bt);
    eprintln!();
}

/// Re-raise SIGTERM with the default handler so the process exits with code 143.
#[cfg(unix)]
fn reraise_sigterm() {
    unsafe {
        // Restore default SIGTERM handler
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        // Re-raise so the process exits with the expected signal status
        libc::raise(libc::SIGTERM);
    }
}
