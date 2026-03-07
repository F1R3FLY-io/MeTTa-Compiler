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
            eprintln!("  avg freed/release:  {:>12.0}", sgc.values_freed_total as f64 / sgc.releases_total as f64);
            eprintln!("  avg promoted/rel:   {:>12.0}", sgc.values_promoted_total as f64 / sgc.releases_total as f64);
            if sgc.values_freed_total == 0 {
                eprintln!("  NOTE: 0 freed values is expected during sequential evaluation —");
                eprintln!("        all values remain reachable from the live environment.");
            }
        }
        eprintln!();
    }
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
    eprintln!("  committed_bytes:    {:>12} ({:.1} MB)", committed, committed as f64 / (1024.0 * 1024.0));
    eprintln!("  gc_threshold:       {:>12} ({:.1} MB)", threshold, threshold as f64 / (1024.0 * 1024.0));
    eprintln!("  commit/threshold:   {:>12.2}", ratio);
    eprintln!("  backpressure_level: {:>12} (max={})", bp_level, gc_allocator::MAX_BACKPRESSURE);
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
    eprintln!("  value_committed:    {:>12} ({:.1} MB)", ps.value_committed_bytes, ps.value_committed_bytes as f64 / (1024.0 * 1024.0));
    eprintln!("  data_pages:         {:>12}", ps.data_page_count);
    eprintln!("  data_committed:     {:>12} ({:.1} MB)", ps.data_committed_bytes, ps.data_committed_bytes as f64 / (1024.0 * 1024.0));
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
            eprintln!("  avg freed/release:  {:>12.0}", sgc.values_freed_total as f64 / sgc.releases_total as f64);
            eprintln!("  avg promoted/rel:   {:>12.0}", sgc.values_promoted_total as f64 / sgc.releases_total as f64);
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
    eprintln!("  interpreter:         {:>12} ({:>5.1}%)", stats.interpreter_executions, pct(stats.interpreter_executions));
    eprintln!("  bytecode VM:         {:>12} ({:>5.1}%)", stats.bytecode_executions, pct(stats.bytecode_executions));
    eprintln!("  JIT stage 1:         {:>12} ({:>5.1}%)", stats.jit1_executions, pct(stats.jit1_executions));
    eprintln!("  JIT stage 2:         {:>12} ({:>5.1}%)", stats.jit2_executions, pct(stats.jit2_executions));
    eprintln!("  total dispatched:    {:>12}", total_dispatched);
    eprintln!();

    // Compilation counts in a tabular grid
    eprintln!("── Compilation Counts ────────────────────────────────────────");
    eprintln!("  {:>16}  {:>10}  {:>10}  {:>10}", "", "triggered", "completed", "failed");
    eprintln!("  {:>16}  {:>10}  {:>10}  {:>10}", "────────────────", "──────────", "──────────", "──────────");
    eprintln!("  {:>16}  {:>10}  {:>10}  {:>10}", "bytecode",
        stats.bytecode_compilations_triggered,
        stats.bytecode_compilations_completed,
        stats.bytecode_compilations_failed);
    eprintln!("  {:>16}  {:>10}  {:>10}  {:>10}", "JIT stage 1",
        stats.jit1_compilations_triggered,
        stats.jit1_compilations_completed,
        stats.jit1_compilations_failed);
    eprintln!("  {:>16}  {:>10}  {:>10}  {:>10}", "JIT stage 2",
        stats.jit2_compilations_triggered,
        stats.jit2_compilations_completed,
        stats.jit2_compilations_failed);
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
        eprintln!("  nondeterminism:     {:>12}", stats.jit1_failures_nondeterminism);
        eprintln!("  unsupported opcode: {:>12}", stats.jit1_failures_unsupported_opcode);
        eprintln!("  compiler init:      {:>12}", stats.jit1_failures_compiler_init);
        eprintln!("  codegen error:      {:>12}", stats.jit1_failures_codegen);
        eprintln!();
    }

    if total_jit2_failures > 0 {
        eprintln!("── JIT Stage 2 Failure Reasons ───────────────────────────────");
        eprintln!("  nondeterminism:     {:>12}", stats.jit2_failures_nondeterminism);
        eprintln!("  unsupported opcode: {:>12}", stats.jit2_failures_unsupported_opcode);
        eprintln!("  compiler init:      {:>12}", stats.jit2_failures_compiler_init);
        eprintln!("  codegen error:      {:>12}", stats.jit2_failures_codegen);
        eprintln!();
    }

    // Per-expression breakdown (top 20 by execution count)
    let per_expr = crate::backend::bytecode::tiered_cache::global_tiered_cache()
        .per_expression_stats();
    if !per_expr.is_empty() {
        let limit = per_expr.len().min(20);
        eprintln!("── Per-Expression Detail (top {} by exec count) ──────────────", limit);
        eprintln!("  {:<18}  {:>8}  {:>10}  {:>10}  {:>10}",
            "hash", "execs", "bytecode", "jit1", "jit2");
        eprintln!("  {:<18}  {:>8}  {:>10}  {:>10}  {:>10}",
            "──────────────────", "────────", "──────────", "──────────", "──────────");
        for entry in per_expr.iter().take(limit) {
            eprintln!("  {:<18}  {:>8}  {:>10}  {:>10}  {:>10}",
                format!("0x{:016x}", entry.expr_hash),
                entry.execution_count,
                entry.bytecode_status,
                entry.jit1_status,
                entry.jit2_status);
        }
        eprintln!();
    }
}

/// Print thread pool statistics to stderr.
///
/// Shows worker counts, queue depths, and throughput metrics for all
/// thread pool subsystems (WorkPool, GcPool).
/// Requires the `track-stats` feature.
#[cfg(feature = "track-stats")]
pub fn print_pool_stats() {
    use crate::backend::models::work_pool::{global_eval_pool, global_compile_pool, work_eval_count};
    use crate::backend::models::gc_pool::global_gc_pool;

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
    eprintln!("  p2_median_runtime:  {:>12.0} ns ({:.2} ms)", ep_median_ns, ep_median_ms);
    eprintln!();

    // ── Compile Pool ──
    let cp = global_compile_pool();
    let cp_active = cp.active_workers();
    let cp_queue = cp.queue_len();
    let cp_median_ns = cp.runtime_tracker().global_median();
    let cp_median_ms = cp_median_ns / 1_000_000.0;

    eprintln!("── Compile Pool (fixed {} workers) ─────────────────────────────", cp.max_threads());
    eprintln!("  active_workers:     {:>12}", cp_active);
    eprintln!("  queue_depth:        {:>12}", cp_queue);
    eprintln!("  p2_median_runtime:  {:>12.0} ns ({:.2} ms)", cp_median_ns, cp_median_ms);
    eprintln!();

    // ── GC Pool ──
    let gp = global_gc_pool();
    let gp_min = gp.min_workers();
    let gp_max = gp.max_workers();
    let gp_active = gp.active_workers();
    let gp_parked = gp_max.saturating_sub(gp_active);
    let gp_releases = gp.session_release_count();

    eprintln!("── GC Pool (Mark-Sweep + Session Release) ─────────────────────");
    eprintln!("  min_workers:        {:>12}", gp_min);
    eprintln!("  max_workers:        {:>12}", gp_max);
    eprintln!("  active_workers:     {:>12}", gp_active);
    eprintln!("  parked_workers:     {:>12}", gp_parked);
    eprintln!("  session_releases:   {:>12}", gp_releases);
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
    eprintln!("  p2_median_runtime:  {:>12.0} ns ({:.2} ms)", crp_median_ns, crp_median_ms);
    eprintln!();
}
