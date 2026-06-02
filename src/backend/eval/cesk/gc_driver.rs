//! E1-a.3 — the dedicated index-GC thread + the §Part 2 quiescence driver.
//!
//! Phase D+E (`docs/cesk-gc/phase-de-concurrent-collector-design.md`) re-founds
//! the index collector as a DEDICATED GC THREAD that drives collection while
//! mutators only poll/self-root/park. E1-a stands up that thread + driver in the
//! OUTPUT-EQUIVALENT regime (true quiescence, `n_threads()==0`), so the
//! infrastructure is validated in the safe regime before E1-c (worker park) and
//! E1-FLIP (gate relax) extend it to collect under FANOUT>0.
//!
//! Behind `gc_allocator::dedicated_gc_enabled()` (env `METTATRON_INDEX_GC_DEDICATED`,
//! default OFF). Default OFF ⇒ the quiescence collection runs INLINE on the
//! calling thread exactly as before (BYTE-IDENTICAL). When ON, the mutator (which
//! has already dropped its `EvalGuard`, so `n_threads()==0` and
//! `active_evaluator_count()==0` at the collection point) hands its already-built
//! structural root set to the GC thread and BLOCKS for completion — output-
//! equivalent, since it would have blocked for the inline collection too.
//!
//! Genuine CESK: the roots are the mutator's structurally-read machine roots ∪
//! reach(E₀) ∪ driver-C, built at the call site (eval/mod.rs:296-307) and HANDED
//! OVER — the GC thread does not *discover* roots, it receives a `Vec<MettaValue>`
//! and marks from it. No registry, no `RootProvider`. The only change vs. inline
//! is WHICH thread runs the (unchanged) `run_collection_if_triggered`.
//!
//! No-hang under FANOUT>0: once any worker has spawned, `worker_ever_spawned()`
//! latches true, so BOTH `should_collect()` at the call site AND `gate_open()`
//! re-checked on the GC thread return false ⇒ the dedicated path backs off to a
//! no-op, exactly as the inline collector is quiescence-only today. No park wait
//! is engaged at E1-a (that is E1-c), so there is nothing to hang on.
//!
//! Concurrency note: the request `Sender` is shared by all mutator threads via a
//! `Mutex` (so the `GcDriver` is `Sync` and can live in a `static`). Each request
//! carries its OWN response channel (created on the mutator's stack), so responses
//! are matched to requests with no stealing — robust for the concurrent callers
//! E1-c will introduce. At E1-a only the sole quiescent mutator (`n_threads()==0`)
//! ever reaches the handoff.

use std::sync::{mpsc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crate::backend::models::gc_allocator::{dedicated_gc_enabled, n_threads};
use crate::backend::models::MettaValue;

/// Response: the bool `run_collection_if_triggered` returned (a cycle ran / was
/// gated). Plumbed back so the mutator's observable behavior matches the inline
/// call (which also discards this bool). `bool` is `Send`, so this needs no
/// `unsafe impl`.
struct GcDriverDone(#[allow(dead_code)] bool);

/// Request sent to the dedicated GC thread.
enum GcDriverRequest {
    /// Collect from these structural roots (MOVED to the GC thread so it owns them
    /// for the whole mark — they ARE the roots the in-place mark reads), then send
    /// the outcome back on the per-request response channel.
    Collect(Vec<MettaValue>, mpsc::Sender<GcDriverDone>),
    /// Graceful shutdown (sent from `Drop`).
    Shutdown,
}

// SAFETY: in index mode a `MettaValue` is a NaN-boxed `Addr` — a plain index into
// the process-global index heap; the referenced nodes are `'static` (owned by the
// heap) and immutable after publish. The dedicated path only ever runs under
// `gc_mode_is_index()` at TRUE QUIESCENCE (`n_threads()==0`, the sole mutator
// blocked on its response), so there is no concurrent reader/writer of those nodes
// while the Vec is in flight; the mutator does not touch the Vec after sending.
// This is the identical `Send` justification the slab `GcRequest` makes
// (`gc_thread.rs`). The embedded `Sender<GcDriverDone>` is genuinely `Send`. The
// channel is never exercised in slab mode (the call site's `dedicated_gc_enabled()`
// / `gate_open()` are false there).
unsafe impl Send for GcDriverRequest {}

struct GcDriver {
    /// `Mutex`-wrapped so `GcDriver: Sync` (the static is shared by all mutators).
    /// Held only for the brief `send`, never across the collection.
    request_tx: Mutex<mpsc::Sender<GcDriverRequest>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for GcDriver {
    fn drop(&mut self) {
        // Best-effort graceful shutdown (explicit teardown / tests; the `static`
        // never drops at process exit, where the thread is simply killed).
        if let Ok(tx) = self.request_tx.lock() {
            let _ = tx.send(GcDriverRequest::Shutdown);
        }
        if let Some(h) = self.handle.lock().ok().and_then(|mut h| h.take()) {
            let _ = h.join();
        }
    }
}

/// `None` ⇒ the thread could not be spawned; callers fall back to inline.
static GLOBAL_GC_DRIVER: OnceLock<Option<GcDriver>> = OnceLock::new();

/// The dedicated GC thread main loop. Owns each request's root Vec for the whole
/// collection, so the roots stay alive exactly while the in-place mark reads them.
fn gc_driver_main(request_rx: mpsc::Receiver<GcDriverRequest>) {
    while let Ok(req) = request_rx.recv() {
        let (roots, resp_tx) = match req {
            GcDriverRequest::Collect(roots, resp_tx) => (roots, resp_tx),
            GcDriverRequest::Shutdown => break,
        };
        // `run_collection_if_triggered` re-checks `gate_open()` (the no-hang
        // backoff under FANOUT>0) then `mark_sweep_if_over_watermark`, which takes
        // `GcInProgressGuard::try_enter()` + the heap `.write()` across mark+sweep.
        // catch_unwind so a panicked cycle reports "did not run" and the thread
        // survives (the mutator unblocks; the next quiescence collection reclaims).
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered(&roots)
        }))
        .unwrap_or(false);
        // `roots` drops HERE, after the cycle — never before the mark completes.
        let _ = resp_tx.send(GcDriverDone(ran)); // ignore if the mutator is gone
    }
}

fn spawn_gc_driver() -> Option<GcDriver> {
    let (request_tx, request_rx) = mpsc::channel::<GcDriverRequest>();
    let handle = thread::Builder::new()
        .name("mettatron-index-gc".to_string())
        .spawn(move || gc_driver_main(request_rx))
        .ok()?; // graceful: None on spawn failure ⇒ caller runs inline
    Some(GcDriver {
        request_tx: Mutex::new(request_tx),
        handle: Mutex::new(Some(handle)),
    })
}

/// Hand `roots` to the GC thread and block for completion.
/// - `Ok(ran)`  — the roots were CONSUMED by the GC thread (the cycle ran / was
///   gated / was safely skipped on thread death); the caller must NOT run anything
///   else with these roots.
/// - `Err(roots)` — the handoff FAILED *before* consuming the roots (no thread /
///   channel closed); the caller runs the inline collection with the returned roots.
///
/// CRUCIAL: a `recv()` failure AFTER a successful send (the GC thread panicked
/// mid-cycle) returns `Ok(false)`, NOT `Err` — the roots are already gone, so we
/// must NEVER fall back to a collection with an EMPTY root set (that would mark
/// nothing and sweep every live value → corruption). A skipped cycle is always
/// safe; the next quiescence collection reclaims.
fn try_drive_blocking(roots: Vec<MettaValue>) -> Result<bool, Vec<MettaValue>> {
    let Some(driver) = GLOBAL_GC_DRIVER.get_or_init(spawn_gc_driver).as_ref() else {
        return Err(roots); // spawn failed → inline fallback with the roots
    };
    let (resp_tx, resp_rx) = mpsc::channel::<GcDriverDone>();
    // Send under the request lock (brief); RELEASE it before waiting so a later
    // mutator can enqueue (moot at E1-a's n_threads()==0, but correct for E1-c).
    {
        let Ok(tx) = driver.request_tx.lock() else {
            return Err(roots); // poisoned (thread panicked) → inline fallback
        };
        match tx.send(GcDriverRequest::Collect(roots, resp_tx)) {
            Ok(()) => {}
            // Thread gone: `SendError` hands the un-sent request back ⇒ recover the
            // roots for the inline fallback.
            Err(mpsc::SendError(GcDriverRequest::Collect(roots, _))) => return Err(roots),
            Err(_) => return Ok(false), // unreachable (only Collect sent); never empty-collect
        }
    }
    // Sent (roots moved to the thread). Wait for done; on recv failure the roots
    // are gone ⇒ skip this cycle (Ok(false)), do NOT collect inline.
    Ok(resp_rx.recv().map(|d| d.0).unwrap_or(false))
}

/// E1-a.3 entry from the quiescence collection point (eval/mod.rs, tier_forced.rs).
///
/// Routes the SAME `roots` to the dedicated GC thread iff `dedicated_gc_enabled()
/// && n_threads()==0`; otherwise (default-OFF, or any handoff failure) runs the
/// identical `run_collection_if_triggered(&roots)` INLINE — byte-identical when
/// the flag is off. The `n_threads()==0` guard makes the quiescence-only contract
/// explicit at the call site (redundant with `gate_open()`'s `active==0`, but
/// self-documenting and the precise boundary E1-c later lifts).
pub(crate) fn collect_quiescence(roots: Vec<MettaValue>) {
    if dedicated_gc_enabled() && n_threads() == 0 {
        if let Err(returned) = try_drive_blocking(roots) {
            // Handoff failed BEFORE consuming roots → inline with the real roots.
            crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered(&returned);
        }
        // Ok(_) ⇒ consumed by the GC thread (ran or safely skipped) — no inline.
    } else {
        crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered(&roots);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_is_lazy_and_idempotent() {
        // The driver is spawned at most once (OnceLock); accessing it twice yields
        // the same instance (or the same None on spawn failure). The spawned thread
        // idles on recv() and is killed at process exit (the static never drops).
        let a = GLOBAL_GC_DRIVER.get_or_init(spawn_gc_driver).is_some();
        let b = GLOBAL_GC_DRIVER.get_or_init(spawn_gc_driver).is_some();
        assert_eq!(a, b, "OnceLock returns the same spawn outcome");
    }

    #[test]
    fn collect_quiescence_default_off_routes_inline() {
        // Default-OFF (env unset, not forced) ⇒ collect_quiescence takes the inline
        // branch (no dedicated thread). Assert the routing PREDICATE only — do NOT
        // call collect_quiescence with a synthetic root set: in the index-gc build
        // that runs a real collection, and an EMPTY root set would mark nothing and
        // sweep every live value in the shared global heap. The inline route is
        // exercised for real by the conformance gate (complete root set, FANOUT=0).
        assert!(
            !dedicated_gc_enabled(),
            "test must run with METTATRON_INDEX_GC_DEDICATED unset/off"
        );
    }
}
