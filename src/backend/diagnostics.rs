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

/// Install SIGTERM, SIGUSR1, and SIGILL diagnostic handlers.
///
/// Idempotent — repeated calls are a single atomic load after first init.
/// Disabled when `METTATRON_NO_SIGNAL_DIAG=1` is set.
///
/// SIGILL is registered with a custom action (via `signal_hook::low_level`)
/// that sets the flag AND restores `SIG_DFL` before returning, so the
/// offending instruction re-executes once and terminates the process via
/// the default disposition (producing a coredump if `RLIMIT_CORE` permits).
/// The watcher thread polls every 50ms and dumps diagnostics if it sees
/// the flag before coredump generation finishes. The flag-then-default
/// chain is deliberate: SIGILL is unrecoverable, and any recover-and-resume
/// approach would loop forever on the same `ud2` instruction.
///
/// This is automatically called from `global_allocator()`, so every code path
/// (binary, tests, benchmarks) gets coverage without explicit setup.
#[cfg(unix)]
pub fn install_signal_handlers() {
    use std::sync::atomic::Ordering;

    INSTALL_ONCE.call_once(|| {
        // Diagnostic-only (env-gated, default off): permit any same-uid process (gdb)
        // to PTRACE_ATTACH under yama `ptrace_scope=1`, so a reliably-reproduced hang
        // can be inspected by attaching to the ALREADY-hung process — gdb cannot LAUNCH
        // it because the debugger's overhead perturbs the timing race away. No effect
        // unless `METTATRON_ALLOW_PTRACE=1`; never enabled in production runs.
        #[cfg(target_os = "linux")]
        if std::env::var("METTATRON_ALLOW_PTRACE").as_deref() == Ok("1") {
            // SAFETY: a single libc prctl with PR_SET_PTRACER_ANY ((unsigned long)-1).
            unsafe {
                libc::prctl(libc::PR_SET_PTRACER, libc::c_ulong::MAX, 0, 0, 0);
            }
        }

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
        let sigill_flag = Arc::new(AtomicBool::new(false));

        // Register signal flags (async-signal-safe: only sets atomics)
        if let Err(e) =
            signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&sigterm_flag))
        {
            eprintln!("[diagnostics] Failed to register SIGTERM handler: {}", e);
            return;
        }
        if let Err(e) =
            signal_hook::flag::register(signal_hook::consts::SIGUSR1, Arc::clone(&sigusr1_flag))
        {
            eprintln!("[diagnostics] Failed to register SIGUSR1 handler: {}", e);
            return;
        }

        // Register SIGILL with custom handler. We can't use `flag::register`
        // on its own because that would loop forever (the offending `ud2`
        // instruction would re-execute after the handler returns, raising
        // SIGILL again, etc). Instead, set the flag AND restore SIG_DFL so
        // the next re-execution terminates the process normally.
        //
        // SIGILL is in signal-hook's FORBIDDEN list (the safe `register`
        // refuses it), so we go through `register_signal_unchecked` to
        // install our handler. SAFETY: the closure performs only
        // async-signal-safe operations — an atomic store and `libc::signal`.
        let sigill_flag_for_handler = Arc::clone(&sigill_flag);
        let register_result = unsafe {
            signal_hook_registry::register_signal_unchecked(
                signal_hook::consts::SIGILL,
                move || {
                    sigill_flag_for_handler.store(true, Ordering::Release);
                    libc::signal(libc::SIGILL, libc::SIG_DFL);
                },
            )
        };
        if let Err(e) = register_result {
            eprintln!("[diagnostics] Failed to register SIGILL handler: {}", e);
            return;
        }

        // Spawn daemon watcher thread (won't prevent process exit)
        thread::Builder::new()
            .name("diag-watcher".to_string())
            .spawn(move || signal_watcher_loop(sigterm_flag, sigusr1_flag, sigill_flag))
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
    sigill_flag: Arc<AtomicBool>,
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

        if sigill_flag.swap(false, Ordering::AcqRel) {
            // SIGILL is non-recoverable: the custom handler has already
            // restored SIG_DFL, so the offending ud2 will re-execute and
            // terminate the process shortly. Dump diagnostics while the
            // kernel is generating the coredump. Do NOT re-raise — the
            // default-disposition path already handles termination.
            dump_diagnostics("SIGILL");
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
    eprintln!(
        "║  Signal: {:<10}  PID: {:<10}  Epoch: {:<14} ║",
        signal_name, pid, timestamp
    );
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!();

    dump_gc_state();
    dump_evaluator_state();
    dump_thread_info();
    dump_watcher_backtrace();

    eprintln!("═══════════════════════ END DIAGNOSTIC DUMP ═══════════════════");
    eprintln!();
}

/// Dump GC state from existing public APIs. Thin wrapper over [`render_gc_state`]
/// so the section is unit-testable (it returns the text instead of writing stderr).
#[cfg(unix)]
fn dump_gc_state() {
    eprint!("{}", render_gc_state());
}

/// Render the GC-state section as a string.
///
/// #275 — the dump is SOURCE-COUPLED to the active GC mode. In index mode it reports the
/// CESK/`IndexHeap` allocator state + the dedicated-collector rendezvous/witness cycle state
/// and MUST NOT present the legacy slab page counters as the authoritative GC state; in slab
/// mode it reports the slab allocator's pages exactly as before. The shared rendezvous booleans
/// (`gc_cycle_in_flight`/`gc_requested`/…) are mode-agnostic and printed in both.
#[cfg(unix)]
fn render_gc_state() -> String {
    use crate::backend::models::gc_allocator;
    use std::fmt::Write as _;

    let mut out = String::with_capacity(1024);

    let in_flight = gc_allocator::gc_cycle_in_flight();
    let requested = gc_allocator::is_gc_requested();
    let disabled = gc_allocator::is_gc_disabled();
    let reachable = gc_allocator::gc_reachable_counter();
    let bp_level = gc_allocator::backpressure_level();
    // The canonical runtime mode flag (always true since the slab store was removed).
    let index_mode = crate::backend::models::metta_value::gc_mode_is_index();

    let _ = writeln!(
        out,
        "── GC State ──────────────────────────────────────────────────"
    );
    let _ = writeln!(
        out,
        "  gc_mode:            {:>12}",
        if index_mode { "index (CESK)" } else { "slab" }
    );
    let _ = writeln!(
        out,
        "  backpressure_level: {:>12} (max={})",
        bp_level,
        gc_allocator::MAX_BACKPRESSURE
    );
    let _ = writeln!(out, "  gc_cycle_in_flight: {:>12}", in_flight);
    let _ = writeln!(out, "  gc_requested:       {:>12}", requested);
    let _ = writeln!(out, "  gc_disabled:        {:>12}", disabled);
    let _ = writeln!(out, "  gc_reachable_ctr:   {:>12}", reachable);
    let _ = writeln!(out);

    // ---- index mode: report the CESK IndexHeap + rendezvous cycle, NOT the slab pages ----
    {
        if index_mode {
            render_index_heap_state(&mut out);
            return out;
        }
    }

    // ---- slab mode (and any non-index build): the legacy slab allocator pages ----
    let alloc = gc_allocator::global_allocator();
    let committed = alloc.committed_bytes();
    let threshold = alloc.gc_threshold();
    let ratio = if threshold > 0 {
        committed as f64 / threshold as f64
    } else {
        0.0
    };
    let _ = writeln!(
        out,
        "  committed_bytes:    {:>12} ({:.1} MB)",
        committed,
        committed as f64 / (1024.0 * 1024.0)
    );
    let _ = writeln!(
        out,
        "  gc_threshold:       {:>12} ({:.1} MB)",
        threshold,
        threshold as f64 / (1024.0 * 1024.0)
    );
    let _ = writeln!(out, "  commit/threshold:   {:>12.2}", ratio);
    let _ = writeln!(out);

    // Page statistics
    let ps = alloc.page_stats();
    let dead_slots = ps.total_bumped_slots.saturating_sub(ps.total_live_slots);
    let occupancy = if ps.total_bumped_slots > 0 {
        ps.total_live_slots as f64 / ps.total_bumped_slots as f64 * 100.0
    } else {
        0.0
    };

    let _ = writeln!(
        out,
        "── Slab Pages ────────────────────────────────────────────────"
    );
    let _ = writeln!(out, "  value_pages:        {:>12}", ps.value_page_count);
    let _ = writeln!(out, "  slots_per_page:     {:>12}", ps.slots_per_page);
    let _ = writeln!(out, "  total_bumped:       {:>12}", ps.total_bumped_slots);
    let _ = writeln!(out, "  total_live:         {:>12}", ps.total_live_slots);
    let _ = writeln!(out, "  dead (bumped-live): {:>12}", dead_slots);
    let _ = writeln!(out, "  occupancy:          {:>11.1}%", occupancy);
    let _ = writeln!(
        out,
        "  value_committed:    {:>12} ({:.1} MB)",
        ps.value_committed_bytes,
        ps.value_committed_bytes as f64 / (1024.0 * 1024.0)
    );
    let _ = writeln!(out, "  data_pages:         {:>12}", ps.data_page_count);
    let _ = writeln!(
        out,
        "  data_committed:     {:>12} ({:.1} MB)",
        ps.data_committed_bytes,
        ps.data_committed_bytes as f64 / (1024.0 * 1024.0)
    );
    let _ = writeln!(out);

    // Session GC statistics (requires track-stats feature)
    #[cfg(feature = "track-stats")]
    {
        let sgc = gc_allocator::session_gc_stats();
        let _ = writeln!(
            out,
            "── Session GC ────────────────────────────────────────────────"
        );
        let _ = writeln!(out, "  sessions_released:  {:>12}", sgc.releases_total);
        let _ = writeln!(out, "  values_freed:       {:>12}", sgc.values_freed_total);
        let _ = writeln!(
            out,
            "  values_promoted:    {:>12}",
            sgc.values_promoted_total
        );
        let _ = writeln!(
            out,
            "  values_scanned:     {:>12}",
            sgc.values_scanned_total
        );
        let _ = writeln!(
            out,
            "  surviving_set_size: {:>12}",
            sgc.last_surviving_set_size
        );
        if sgc.releases_total > 0 {
            let _ = writeln!(
                out,
                "  avg freed/release:  {:>12.0}",
                sgc.values_freed_total as f64 / sgc.releases_total as f64
            );
            let _ = writeln!(
                out,
                "  avg promoted/rel:   {:>12.0}",
                sgc.values_promoted_total as f64 / sgc.releases_total as f64
            );
            if sgc.values_freed_total == 0 {
                let _ = writeln!(
                    out,
                    "  NOTE: 0 freed values is expected during sequential evaluation —"
                );
                let _ = writeln!(
                    out,
                    "        all values remain reachable from the live environment."
                );
            }
        }
        let _ = writeln!(out);
    }

    out
}

/// #275 — render the CESK/index allocator state for the diagnostic dump (index mode only).
///
/// Reports the `IndexHeap` allocator telemetry (committed / live / old-live / young-alloc /
/// nursery backpressure) and the dedicated concurrent collector's rendezvous CYCLE state
/// (generation, started-gate, witness gate, and the witness-slot occupancy summary). The latter
/// is the E1 liveness lever: a nonzero `occupied_unpublished` while the GC is idle
/// (`gc_cycle_in_flight=false`, `gc_requested=false`) is a mutator parked for a cycle no driver
/// will close. The `IndexHeap` is read with `try_read()` so the dump NEVER blocks the diagnostic
/// watcher thread on the heap lock (a collection holding the write lock prints a clear note
/// instead of deadlocking the dump).
#[cfg(unix)]
fn render_index_heap_state(out: &mut String) {
    use crate::backend::eval::cesk::index_heap::global_index_heap;
    use crate::backend::models::gc_allocator;
    use std::fmt::Write as _;

    let _ = writeln!(
        out,
        "── Index Heap (CESK) ─────────────────────────────────────────"
    );
    match global_index_heap().try_read() {
        Ok(heap) => {
            let committed = heap.committed_bytes();
            let live = heap.live_bytes();
            let old_live = heap.old_live_bytes();
            let young = heap.young_alloc_bytes();
            let nursery_pending = heap.nursery_full_pending();
            let occupancy = if committed > 0 {
                live as f64 / committed as f64 * 100.0
            } else {
                0.0
            };
            let _ = writeln!(
                out,
                "  committed_bytes:    {:>12} ({:.1} MB)",
                committed,
                committed as f64 / (1024.0 * 1024.0)
            );
            let _ = writeln!(
                out,
                "  live_bytes:         {:>12} ({:.1} MB)",
                live,
                live as f64 / (1024.0 * 1024.0)
            );
            let _ = writeln!(
                out,
                "  old_live_bytes:     {:>12} ({:.1} MB)",
                old_live,
                old_live as f64 / (1024.0 * 1024.0)
            );
            let _ = writeln!(
                out,
                "  young_alloc_bytes:  {:>12} ({:.1} MB)",
                young,
                young as f64 / (1024.0 * 1024.0)
            );
            let _ = writeln!(out, "  live/committed:     {:>11.1}%", occupancy);
            let _ = writeln!(out, "  nursery_full_pending:{:>11}", nursery_pending);
        }
        Err(_) => {
            let _ = writeln!(
                out,
                "  (index heap is write-locked — a collection is in progress; stats are"
            );
            let _ = writeln!(
                out,
                "   read with try_read to avoid blocking the diagnostic dump)"
            );
        }
    }
    let _ = writeln!(out);

    // Dedicated collector rendezvous / cycle state (the E1 liveness diagnosis lever).
    let gen = gc_allocator::current_cycle_gen();
    let started = gc_allocator::current_cycle_started();
    let witness_ok = gc_allocator::current_witness_ok();
    let (total, occupied, occupied_unpublished) = gc_allocator::witness_directory_summary(gen);
    let _ = writeln!(
        out,
        "── GC Cycle / Rendezvous (CESK) ──────────────────────────────"
    );
    let _ = writeln!(out, "  cycle_gen:          {:>12}", gen);
    let _ = writeln!(out, "  cycle_started:      {:>12}", started);
    let _ = writeln!(out, "  witness_ok:         {:>12}", witness_ok);
    let _ = writeln!(out, "  witness_slots:      {:>12}", total);
    let _ = writeln!(out, "  occupied:           {:>12}", occupied);
    let _ = writeln!(
        out,
        "  occupied_unpublished:{:>11}  (occupied ∧ published<cycle_gen — parked, not re-rooted this cycle)",
        occupied_unpublished
    );
    // GC wait-site occupancy — names the permanently-blocked site at a hang (Inc B).
    let (gate, park, straddle) = gc_allocator::gc_wait_site_occupancy();
    let _ = writeln!(
        out,
        "  gc_wait gate:       {:>12}  (worker_wait_for_resume / WorkerEnter)",
        gate
    );
    let _ = writeln!(
        out,
        "  gc_wait park:       {:>12}  (worker_resume_wait_for_cycle)",
        park
    );
    let _ = writeln!(
        out,
        "  gc_wait straddle:   {:>12}  (reacquire_eval_guard_after_safepoint_full)",
        straddle
    );
    let _ = writeln!(out);
}

/// Dump evaluator state.
#[cfg(unix)]
fn dump_evaluator_state() {
    use crate::backend::models::gc_allocator;

    let active = gc_allocator::active_evaluator_count();

    eprintln!("── Evaluator State ───────────────────────────────────────────");
    eprintln!("  active_evaluators:  {:>12}", active);
    eprintln!();

    // Work pool (eval scheduler) state. `queue_depth > 0` while all workers are
    // idle/parked pins a submitted task that no worker ever dequeued — the
    // dispatch/wakeup-gap signature of a stuck parallel-collapse completion
    // (`remaining` never reaches 0, parent pumps forever). `queue_depth == 0`
    // means the missing work is instead blocked INSIDE a worker closure.
    let pool = crate::backend::models::work_pool::global_eval_pool();
    eprintln!("── Work Pool (eval scheduler) ────────────────────────────────");
    eprintln!("  queue_depth:        {:>12}", pool.queue().len());
    eprintln!("  active_workers:     {:>12}", pool.active_workers());
    eprintln!("  max_threads:        {:>12}", pool.max_threads());
    eprintln!();
}

/// Dump thread information from /proc/self/task/ (Linux-specific).
#[cfg(target_os = "linux")]
fn dump_thread_info() {
    eprintln!("── Thread Table ──────────────────────────────────────────────");
    eprintln!(
        "  {:>7}  {:<24}  {:<12}  {}",
        "TID", "NAME", "STATE", "WCHAN"
    );
    eprintln!(
        "  {:>7}  {:<24}  {:<12}  {}",
        "───────", "────────────────────────", "────────────", "──────────────────────"
    );

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

// ── User-facing stats printing (all platforms) ─────────────────────────

/// Print GC statistics to stderr.
///
/// Uses the same data sources as the signal-triggered diagnostic dump,
/// but formatted for post-run analysis rather than livelock debugging.
/// Requires the `track-stats` feature.
#[cfg(feature = "track-stats")]
pub fn print_gc_stats() {
    use crate::backend::models::gc_allocator;

    let alloc = gc_allocator::global_allocator();
    let committed = alloc.committed_bytes();
    let threshold = alloc.gc_threshold();
    let total_allocs = gc_allocator::alloc_count_snapshot();
    let bp_level = gc_allocator::backpressure_level();
    let in_flight = gc_allocator::gc_cycle_in_flight();
    let requested = gc_allocator::is_gc_requested();
    let disabled = gc_allocator::is_gc_disabled();

    let ratio = if threshold > 0 {
        committed as f64 / threshold as f64
    } else {
        0.0
    };

    eprintln!();
    eprintln!("── GC Statistics ─────────────────────────────────────────────");
    eprintln!("  total_allocations:  {:>12}", total_allocs);
    eprintln!(
        "  committed_bytes:    {:>12} ({:.1} MB)",
        committed,
        committed as f64 / (1024.0 * 1024.0)
    );
    eprintln!(
        "  gc_threshold:       {:>12} ({:.1} MB)",
        threshold,
        threshold as f64 / (1024.0 * 1024.0)
    );
    eprintln!("  commit/threshold:   {:>12.2}", ratio);
    eprintln!(
        "  backpressure_level: {:>12} (max={})",
        bp_level,
        gc_allocator::MAX_BACKPRESSURE
    );
    eprintln!("  gc_cycle_in_flight: {:>12}", in_flight);
    eprintln!("  gc_requested:       {:>12}", requested);
    eprintln!("  gc_disabled:        {:>12}", disabled);
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
    eprintln!(
        "  value_committed:    {:>12} ({:.1} MB)",
        ps.value_committed_bytes,
        ps.value_committed_bytes as f64 / (1024.0 * 1024.0)
    );
    eprintln!("  data_pages:         {:>12}", ps.data_page_count);
    eprintln!(
        "  data_committed:     {:>12} ({:.1} MB)",
        ps.data_committed_bytes,
        ps.data_committed_bytes as f64 / (1024.0 * 1024.0)
    );
    eprintln!();

    // Session GC statistics (requires track-stats feature)
    #[cfg(feature = "track-stats")]
    {
        let sgc = gc_allocator::session_gc_stats();
        eprintln!("── Session GC ────────────────────────────────────────────────");
        eprintln!("  sessions_released:  {:>12}", sgc.releases_total);
        eprintln!("  values_freed:       {:>12}", sgc.values_freed_total);
        eprintln!("  values_promoted:    {:>12}", sgc.values_promoted_total);
        eprintln!("  values_scanned:     {:>12}", sgc.values_scanned_total);
        eprintln!("  surviving_set_size: {:>12}", sgc.last_surviving_set_size);
        if sgc.releases_total > 0 {
            eprintln!(
                "  avg freed/release:  {:>12.0}",
                sgc.values_freed_total as f64 / sgc.releases_total as f64
            );
            eprintln!(
                "  avg promoted/rel:   {:>12.0}",
                sgc.values_promoted_total as f64 / sgc.releases_total as f64
            );
            if sgc.values_freed_total == 0 {
                eprintln!("  NOTE: 0 freed values is expected during sequential evaluation —");
                eprintln!("        all values remain reachable from the live environment.");
            }
        }
        eprintln!();
    }
}

/// Print tiered compilation statistics to stderr.
///
/// Shows execution distribution across interpreter/bytecode/JIT tiers
/// and compilation trigger/completion/failure counts.
/// Requires the `track-stats` feature.
#[cfg(feature = "track-stats")]
pub fn print_tier_stats() {
    let stats = crate::backend::bytecode::tiered_cache::global_tiered_cache().stats();

    eprintln!();
    eprintln!("── Tiered Compilation Statistics ──────────────────────────────");
    eprintln!("  expressions_tracked: {:>12}", stats.expressions_tracked);
    eprintln!("  total_executions:    {:>12}", stats.total_executions);
    eprintln!();

    // Execution distribution with percentages
    let total_dispatched = stats.interpreter_executions
        + stats.bytecode_executions
        + stats.jit1_executions
        + stats.jit2_executions;

    let pct = |n: u64| -> f64 {
        if total_dispatched > 0 {
            n as f64 / total_dispatched as f64 * 100.0
        } else {
            0.0
        }
    };

    eprintln!("── Execution Distribution ────────────────────────────────────");
    eprintln!(
        "  interpreter:         {:>12} ({:>5.1}%)",
        stats.interpreter_executions,
        pct(stats.interpreter_executions)
    );
    eprintln!(
        "  bytecode VM:         {:>12} ({:>5.1}%)",
        stats.bytecode_executions,
        pct(stats.bytecode_executions)
    );
    eprintln!(
        "  JIT stage 1:         {:>12} ({:>5.1}%)",
        stats.jit1_executions,
        pct(stats.jit1_executions)
    );
    eprintln!(
        "  JIT stage 2:         {:>12} ({:>5.1}%)",
        stats.jit2_executions,
        pct(stats.jit2_executions)
    );
    eprintln!("  total dispatched:    {:>12}", total_dispatched);
    eprintln!();

    // Compilation counts in a tabular grid
    eprintln!("── Compilation Counts ────────────────────────────────────────");
    eprintln!(
        "  {:>16}  {:>10}  {:>10}  {:>10}",
        "", "triggered", "completed", "failed"
    );
    eprintln!(
        "  {:>16}  {:>10}  {:>10}  {:>10}",
        "────────────────", "──────────", "──────────", "──────────"
    );
    eprintln!(
        "  {:>16}  {:>10}  {:>10}  {:>10}",
        "bytecode",
        stats.bytecode_compilations_triggered,
        stats.bytecode_compilations_completed,
        stats.bytecode_compilations_failed
    );
    eprintln!(
        "  {:>16}  {:>10}  {:>10}  {:>10}",
        "JIT stage 1",
        stats.jit1_compilations_triggered,
        stats.jit1_compilations_completed,
        stats.jit1_compilations_failed
    );
    eprintln!(
        "  {:>16}  {:>10}  {:>10}  {:>10}",
        "JIT stage 2",
        stats.jit2_compilations_triggered,
        stats.jit2_compilations_completed,
        stats.jit2_compilations_failed
    );
    eprintln!();

    // JIT failure reason breakdown (only if any JIT failures exist)
    let total_jit1_failures = stats.jit1_failures_nondeterminism
        + stats.jit1_failures_unsupported_opcode
        + stats.jit1_failures_compiler_init
        + stats.jit1_failures_codegen;
    let total_jit2_failures = stats.jit2_failures_nondeterminism
        + stats.jit2_failures_unsupported_opcode
        + stats.jit2_failures_compiler_init
        + stats.jit2_failures_codegen;

    if total_jit1_failures > 0 {
        eprintln!("── JIT Stage 1 Failure Reasons ───────────────────────────────");
        eprintln!(
            "  nondeterminism:     {:>12}",
            stats.jit1_failures_nondeterminism
        );
        eprintln!(
            "  unsupported opcode: {:>12}",
            stats.jit1_failures_unsupported_opcode
        );
        eprintln!(
            "  compiler init:      {:>12}",
            stats.jit1_failures_compiler_init
        );
        eprintln!("  codegen error:      {:>12}", stats.jit1_failures_codegen);
        eprintln!();
    }

    if total_jit2_failures > 0 {
        eprintln!("── JIT Stage 2 Failure Reasons ───────────────────────────────");
        eprintln!(
            "  nondeterminism:     {:>12}",
            stats.jit2_failures_nondeterminism
        );
        eprintln!(
            "  unsupported opcode: {:>12}",
            stats.jit2_failures_unsupported_opcode
        );
        eprintln!(
            "  compiler init:      {:>12}",
            stats.jit2_failures_compiler_init
        );
        eprintln!("  codegen error:      {:>12}", stats.jit2_failures_codegen);
        eprintln!();
    }

    // Per-expression breakdown (top 20 by execution count)
    let per_expr =
        crate::backend::bytecode::tiered_cache::global_tiered_cache().per_expression_stats();
    if !per_expr.is_empty() {
        let limit = per_expr.len().min(20);
        eprintln!(
            "── Per-Expression Detail (top {} by exec count) ──────────────",
            limit
        );
        eprintln!(
            "  {:<18}  {:>8}  {:>10}  {:>10}  {:>10}",
            "hash", "execs", "bytecode", "jit1", "jit2"
        );
        eprintln!(
            "  {:<18}  {:>8}  {:>10}  {:>10}  {:>10}",
            "──────────────────", "────────", "──────────", "──────────", "──────────"
        );
        for entry in per_expr.iter().take(limit) {
            eprintln!(
                "  {:<18}  {:>8}  {:>10}  {:>10}  {:>10}",
                format!("0x{:016x}", entry.expr_hash),
                entry.execution_count,
                entry.bytecode_status,
                entry.jit1_status,
                entry.jit2_status
            );
        }
        eprintln!();
    }
}

/// Print thread pool statistics to stderr.
///
/// Shows worker counts, queue depths, and throughput metrics for all
/// thread pool subsystems (WorkPool and the Cron pool).
/// Requires the `track-stats` feature.
#[cfg(feature = "track-stats")]
pub fn print_pool_stats() {
    use crate::backend::models::work_pool::{
        global_compile_pool, global_eval_pool, work_eval_count,
    };

    // ── Eval Pool ──
    let ep = global_eval_pool();
    let ep_min = ep.min_threads();
    let ep_max = ep.max_threads();
    let ep_active = ep.active_workers();
    let ep_parked = ep_max.saturating_sub(ep_active);
    let ep_queue = ep.queue_len();
    let ep_evals = work_eval_count();
    let ep_median_ns = ep.runtime_tracker().global_median();
    let ep_median_ms = ep_median_ns / 1_000_000.0;

    eprintln!();
    eprintln!("── Eval Pool (parallel nondeterministic branches) ──────────────");
    eprintln!("  min_threads:        {:>12}", ep_min);
    eprintln!("  max_threads:        {:>12}", ep_max);
    eprintln!("  active_workers:     {:>12}", ep_active);
    eprintln!("  parked_workers:     {:>12}", ep_parked);
    eprintln!("  queue_depth:        {:>12}", ep_queue);
    eprintln!("  parallel_branches:  {:>12}", ep_evals);
    eprintln!(
        "  p2_median_runtime:  {:>12.0} ns ({:.2} ms)",
        ep_median_ns, ep_median_ms
    );
    eprintln!();

    // ── Compile Pool ──
    let cp = global_compile_pool();
    let cp_active = cp.active_workers();
    let cp_queue = cp.queue_len();
    let cp_median_ns = cp.runtime_tracker().global_median();
    let cp_median_ms = cp_median_ns / 1_000_000.0;

    eprintln!(
        "── Compile Pool (fixed {} workers) ─────────────────────────────",
        cp.max_threads()
    );
    eprintln!("  active_workers:     {:>12}", cp_active);
    eprintln!("  queue_depth:        {:>12}", cp_queue);
    eprintln!(
        "  p2_median_runtime:  {:>12.0} ns ({:.2} ms)",
        cp_median_ns, cp_median_ms
    );
    eprintln!();

    // ── Cron Pool ──
    use crate::backend::models::gc_cron::cron_work_pool;

    let crp = cron_work_pool();
    let crp_min = crp.min_threads();
    let crp_max = crp.max_threads();
    let crp_active = crp.active_workers();
    let crp_parked = crp_max.saturating_sub(crp_active);
    let crp_queue = crp.queue_len();
    let crp_median_ns = crp.runtime_tracker().global_median();
    let crp_median_ms = crp_median_ns / 1_000_000.0;

    eprintln!("── Cron Pool (GC scheduling + counter sync) ────────────────────");
    eprintln!("  min_threads:        {:>12}", crp_min);
    eprintln!("  max_threads:        {:>12}", crp_max);
    eprintln!("  active_workers:     {:>12}", crp_active);
    eprintln!("  parked_workers:     {:>12}", crp_parked);
    eprintln!("  queue_depth:        {:>12}", crp_queue);
    eprintln!(
        "  p2_median_runtime:  {:>12.0} ns ({:.2} ms)",
        crp_median_ns, crp_median_ms
    );
    eprintln!();
}

#[cfg(all(test, unix))]
mod index_dump_coupling_tests {
    use super::*;

    /// #275 source-coupling: in index mode the diagnostic GC-state dump must report the
    /// CESK `IndexHeap` allocator + the dedicated-collector rendezvous cycle, and must NOT
    /// present the legacy slab "Slab Pages" counters as the authoritative GC state.
    #[test]
    fn index_mode_dump_reports_index_heap_not_slab_pages() {
        // An `index-gc` build runs index mode by construction.
        // Ensure the global index heap is initialized so the telemetry path is exercised.
        let _ = crate::backend::eval::cesk::index_heap::global_index_heap();

        let report = render_gc_state();

        assert!(
            report.contains("gc_mode:") && report.contains("index (CESK)"),
            "index-mode dump must label the active mode as index; got:\n{report}"
        );
        assert!(
            report.contains("── Index Heap (CESK) ──"),
            "index-mode dump must report IndexHeap/CESK allocator state; got:\n{report}"
        );
        assert!(
            report.contains("── GC Cycle / Rendezvous (CESK) ──"),
            "index-mode dump must report the dedicated-collector rendezvous/witness cycle; got:\n{report}"
        );
        assert!(
            report.contains("occupied_unpublished:"),
            "index-mode dump must include the stranded-parker witness indicator; got:\n{report}"
        );
        assert!(
            !report.contains("Slab Pages"),
            "index-mode dump must NOT present legacy slab page counters as the authoritative \
             GC state; got:\n{report}"
        );
    }
}
