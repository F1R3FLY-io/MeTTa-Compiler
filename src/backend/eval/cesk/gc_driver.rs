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
    /// the outcome back on the per-request response channel. (QUIESCENCE path —
    /// `n_threads()==0`, the sole mutator blocked on the reply.)
    Collect(Vec<MettaValue>, mpsc::Sender<GcDriverDone>),
    /// E1-c (FANOUT>0): run the §Part-2 rendezvous — wait for every active mutator
    /// to park + self-root, drain their structural machines (∪ driver-C) and
    /// collect, then resume them via the cycle-generation bump + `resume_workers`.
    /// Fire-and-forget: it carries NO payload (the roots are pulled from
    /// `WORKER_ROOT_BUFFER`, not sent) and sends NO reply (the parked workers
    /// resume on the gen bump, not on a channel) — so it adds nothing to the
    /// `unsafe impl Send` obligation, which is solely about `Collect`'s Vec.
    CollectRendezvous,
    /// Graceful shutdown (sent from `Drop`).
    Shutdown,
}

// SAFETY: the ONLY non-auto-`Send` payload is `Collect`'s `Vec<MettaValue>`
// (`CollectRendezvous`/`Shutdown` carry no payload). In index mode a `MettaValue`
// is a NaN-boxed `Addr` — a plain index into the process-global index heap; the
// referenced nodes are `'static` (owned by the heap) and immutable after publish.
// The `Collect` (quiescence) path only ever runs under `gc_mode_is_index()` at TRUE
// QUIESCENCE (`n_threads()==0`, the sole mutator blocked on its response), so there
// is no concurrent reader/writer of those nodes while the Vec is in flight; the
// mutator does not touch the Vec after sending. (The `CollectRendezvous` FANOUT>0
// path sends NO Vec — its roots are drained from `WORKER_ROOT_BUFFER` on the GC
// thread AFTER all mutators have parked, so it adds nothing to this obligation.)
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
        match req {
            GcDriverRequest::Shutdown => break,
            // QUIESCENCE (E1-a.3): synchronous handoff — collect from the mutator's
            // roots, reply with the outcome. The mutator is blocked on `resp_tx`.
            GcDriverRequest::Collect(roots, resp_tx) => {
                // `run_collection_if_triggered` re-checks `gate_open()` (the no-hang
                // backoff under FANOUT>0) then `mark_sweep_if_over_watermark`, which
                // takes `GcInProgressGuard::try_enter()` + the heap `.write()` across
                // mark+sweep. catch_unwind so a panicked cycle reports "did not run"
                // and the thread survives (the mutator unblocks; the next quiescence
                // collection reclaims). `roots` drops HERE, after the cycle — never
                // before the mark completes.
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered(&roots)
                }))
                .unwrap_or(false);
                let _ = resp_tx.send(GcDriverDone(ran)); // ignore if the mutator is gone
            }
            // E1-c (FANOUT>0): the §Part-2 rendezvous — fire-and-forget; the parked
            // workers resume via the cycle-gen bump, not a reply channel.
            GcDriverRequest::CollectRendezvous => gc_driver_rendezvous_cycle(),
        }
    }
}

/// E1-c (FANOUT>0): the dedicated GC thread's §Part-2 rendezvous cycle. The
/// triggering mutator has already `request_gc()`'d (so every active mutator polls
/// `is_gc_requested()` at its next safepoint, self-roots its machine ∪ E₀ into
/// `WORKER_ROOT_BUFFER`, and parks). Here the GC thread — which is NOT a mutator, so
/// there is no "the requestor must also pump the dispatch" deadlock (the rejected
/// Phase-D rendezvous) — drives the cycle:
///
/// ```text
///   (2) try_enter GcInProgressGuard            // become the sole collector; close admission
///   (3) n := n_threads(); set_n_threads_at_snapshot(n)   // VESTIGIAL post-flip (oracle/liveness)
///       cur_gen := current_cycle_gen()         // AFTER admission (under _gip) — the witness cycle
///   (4) snap := snapshot_witness(cur_gen)
///       requestor_wait_for_all_reified_parked(&snap, cur_gen)   // WITNESS gate: ∀ occupied slot
///       set_current_witness_ok(true)           //   published>=cur_gen OR acquired>cur_gen (live-re-walk)
///   (5) roots := drain(WORKER_ROOT_BUFFER) ∪ collect_safepoint_roots() ∪ collect_live_env_anchors()  // ∪ driver-C
///   (6) run_collection_if_triggered(roots)     // GATED: gate_open_rendezvous() reads current_witness_ok()
///   (7) end_rendezvous_cycle()                 // bump GC_CYCLE_GEN + reset + clear witness_ok (releases resume)
///   (8) drop _gip                              // GC_IN_PROGRESS=false (wakes enter-parkers)
///   (9) resume_workers()                       // GC_REQUESTED=false + notify RESUME_CONDVAR
/// ```
///
/// Until **E1-FLIP**, `gate_open()` still requires `!worker_ever_spawned()`, so under
/// FANOUT>0 step (6) backs off to a no-op — but the rendezvous (park/drain/resume)
/// still runs end-to-end, which is exactly what the V1/V4 gate exercises. The cleanup
/// (7)-(9) runs even if (6) panics (catch_unwind), so parked workers are ALWAYS
/// released — a panicked cycle never wedges the mutators.
///
/// E₀ is covered without the GC thread holding an env handle: every parked worker
/// self-roots its machine via `collect_machine_roots_live` (⊇ `collect_persistent_roots`,
/// i.e. E₀), and `n ≥ 1` always (the trigger itself parks), so E₀ is in the drained
/// buffer. driver-C (`SAFEPOINT_ROOTS`, the batch-finisher F1 roots) is read directly.
fn gc_driver_rendezvous_cycle() {
    use crate::backend::models::gc_allocator as ga;
    // (2) admission: become the sole collector. Brief yield-retry if a slab cron /
    // session-release path momentarily holds GC_IN_PROGRESS (design Part-12 #6).
    let _gip = loop {
        match ga::GcInProgressGuard::try_enter() {
            Some(g) => break g,
            None => std::thread::yield_now(),
        }
    };
    // (3) snapshot the per-thread count AFTER admission closed (so a thread entering
    // after this point parks at EvalGuard::enter and is excluded), then (4) wait for
    // all n to park + publish their self-roots.
    //
    // E1-FLIP Path B V4 — THE ATOMIC FLIP (witness SOLE gate). The fungible
    // parked-count (`requestor_wait_for_parked_count(n)`) is REPLACED by the per-slot
    // WITNESS predicate: the sweep proceeds only when every OCCUPIED witness slot was
    // STAMPED this cycle by a genuine reified park (`note_reified_park`, the SOLE
    // published-setter). See docs/cesk-gc/e1-flip-pathB-v2-impl.md §Step 2.
    let n = ga::n_threads();
    // Pin 1 (VESTIGIAL-but-harmless): `n`/`set_n_threads_at_snapshot` are no longer the
    // safety gate post-flip (the witness is). Kept computed-and-published because the
    // D5 oracle / liveness backstops may read the snapshot `n`. Taken AFTER admission
    // closed (try_enter above), BEFORE the witness wait.
    ga::set_n_threads_at_snapshot(n);
    // `cur_gen` MUST be read AFTER admission closed (under `_gip`) so the witness
    // predicate (`published>=cur_gen OR acquired>cur_gen`) and the snapshot agree on
    // the cycle a parked thread must have stamped. A thread that enters after this
    // point parks at EvalGuard::enter (admission-blocked) and acquires `acquired>cur_gen`
    // ⇒ excluded from the wait (S3).
    let cur_gen = ga::current_cycle_gen();
    // (4) WITNESS WAIT: block until every occupied slot satisfies the strict-`>`
    // predicate for `cur_gen` (LIVE-RE-WALK each wake — A-straddle-2). The snapshot is
    // a non-empty hint; the authoritative scan is the live re-walk inside the wait.
    let snap = ga::snapshot_witness(cur_gen);
    ga::requestor_wait_for_all_reified_parked(&snap, cur_gen);
    // Publish the witness-satisfied flag — the SOLE thing `gate_open_rendezvous` reads.
    // Set true only AFTER the wait returns; cleared at `end_rendezvous_cycle`.
    ga::set_current_witness_ok(true);
    // (5) the structural root union: parked workers' machines (∪ E₀) + driver-C +
    // the live parallel-dispatch fan-out + the B2′ global live-env (E₀) registry.
    let mut roots: Vec<MettaValue> = Vec::new();
    ga::drain_worker_root_buffer(&mut roots);
    ga::collect_safepoint_roots(&mut roots);
    // E1-FLIP Path B V4 (B2′): walk EVERY live env's persistent E₀ roots — participant-
    // independently — so E₀ is covered even when no Trampoline participant happens to be
    // parked (C-0b: a trigger that finished-via-TierLeaf + all workers finished leaves no
    // Trampoline participant). `#[cfg(index-gc)]`: the registry + register_live_env sites
    // are index-only; in slab nothing ever registers, so the walk is empty.
    #[cfg(feature = "index-gc")]
    ga::collect_live_env_anchors(&mut roots);
    // E1-FLIP / CEX-1 (D2): walk the live dispatch fan-out (branch INPUTS +
    // completed OUTPUTS) structurally on the GC thread — park-timing-independently,
    // so a worker that is admission-blocked at `EvalGuard::enter` or not-yet-started
    // (never a rendezvous participant, never self-rooted) still has its captured
    // `branch_expr`/`branch_bindings` + its slot's results covered. This is the
    // structural replacement for the slab `ParallelDispatchRootProvider` walk that
    // A5 deleted from `ROOT_REGISTRY` without re-homing — the CEX-1 residual bug.
    // `#[cfg(index-gc)]`: the anchor (`LIVE_DISPATCHES`) and the `register_live_dispatch`
    // sites are index-only; in slab nothing ever registers, so the walk is empty.
    #[cfg(feature = "index-gc")]
    ga::collect_live_dispatch_anchors(&mut roots);
    // (5b) E1-FLIP / CEX-1 (D5): the PERMANENT rendezvous-union machine-equivalence
    // oracle — assert the drained union is complete BEFORE the sweep. Zero-cost in
    // release (cfg'd out). A future forgotten thread-local source / unregistered
    // dispatch trips a NAMED debug panic here, not a silent corruption.
    #[cfg(all(feature = "index-gc", debug_assertions))]
    assert_rendezvous_union_complete(&roots, n);
    // (6) collect (catch_unwind so the cleanup below ALWAYS releases parked workers).
    // E1-FLIP: the RENDEZVOUS entry — gates on gate_open_rendezvous (completeness
    // witness, NOT !worker_ever_spawned which is false here) + labels the cycle
    // "rendezvous" (side-Box frees deferred; parked workers may hold laundered refs).
    // Pre-FLIP (dedicated default OFF) this driver is unreachable; post-FLIP it is the
    // FANOUT>0 collect that actually sweeps.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered_rendezvous(&roots)
    }));
    // `roots` drops HERE, after the cycle — never before the mark completes.
    drop(roots);
    // (7) END the cycle: bump GC_CYCLE_GEN + reset parked-count/buffer (releases the
    // parked workers' gen-gated resume-wait); (8) drop _gip (wakes enter-parkers);
    // (9) resume_workers (clears GC_REQUESTED + notifies RESUME_CONDVAR). Order:
    // gen-bump BEFORE the resume notify so a woken worker re-checks an advanced gen.
    ga::end_rendezvous_cycle();
    drop(_gip);
    ga::resume_workers();
}

/// E1-FLIP / CEX-1 (D5) — the PERMANENT rendezvous-union machine-equivalence oracle.
/// Called by `gc_driver_rendezvous_cycle` AFTER the drain (`WORKER_ROOT_BUFFER` ∪
/// `collect_safepoint_roots` ∪ `collect_live_dispatch_anchors`) and BEFORE the sweep.
/// Asserts the two completeness obligations of the concurrent completeness theorem:
///
///   (a) **dispatch-fan-out coverage** — every CURRENTLY-registered live dispatch's
///       INPUT∪OUTPUT `Addr`s are present in the drained `roots`. An independent
///       re-walk (`snapshot_live_dispatch_witness`, no pruning) must be ⊆ `roots`.
///       A registered dispatch the driver failed to walk (a future regression that
///       drops the D2 `collect_live_dispatch_anchors` call) is a NAMED panic here.
///
///   (b) **participant coverage** — E1-FLIP Path B V4: the PER-SLOT WITNESS predicate
///       (`all_occupied_slots_satisfied(cur_gen)`), NOT the fungible parked-count.
///       Every OCCUPIED witness slot must satisfy `published>=cur_gen OR acquired>cur_gen`
///       — the SAME strict-`>` live-re-walk `requestor_wait_for_all_reified_parked` used.
///       A per-slot violator means a counted-AND-occupied mutator never reached a genuine
///       reified park to `note_reified_park` this cycle, so its unpublished machine is NOT
///       in the drained union → under-mark. (The fungible `workers_parked_for_gc() >= n`
///       is no longer the gate: a finisher's bump could "cover" for a parent's not-yet-
///       published machine — exactly the publish-timing UAF the witness fixes.)
///
/// The thread-local half of D1 (the 4 caches + binding-capture + K-spine folded into
/// `collect_global_anchors`/`collect_machine_roots*`) is checked end-to-end by the
/// EXISTING A4.3 midloop oracle (eval_loop.rs ~3915), which proves the FULL structural
/// machine reader ⊇ the independently-discovered `root_set` over LIVE S/C/K. Together
/// (a)+(b)+A4.3 discharge `reachable(R) ⊇ every live value`. Debug-only; zero-cost in
/// release (the call site is `#[cfg(debug_assertions)]`). `#[cfg(index-gc)]`: the
/// dispatch-witness helper it calls is index-only (the anchor is too).
#[cfg(all(feature = "index-gc", debug_assertions))]
fn assert_rendezvous_union_complete(roots: &[MettaValue], n_snapshot: u32) {
    use crate::backend::models::gc_allocator as ga;
    // `inner_ptr()` is an inherent method on `MettaValue` (metta_value.rs:1275), so no
    // `MettaValueTrait` import is needed.

    // (a) dispatch-fan-out coverage. `roots` is the union actually fed to the mark
    // (it already contains the `collect_live_dispatch_anchors` walk). The witness is
    // an INDEPENDENT re-walk (no pruning) of the SAME registry — every witnessed
    // `Addr` must appear in `roots`. (At the rendezvous all participants have parked/
    // finished, so each handle's `results` is stable across the two walks.)
    let (witness_vals, live_handles) = ga::snapshot_live_dispatch_witness();
    if !witness_vals.is_empty() {
        let mut fed: Vec<usize> = roots.iter().map(|v| v.inner_ptr() as usize).collect();
        fed.sort_unstable();
        fed.dedup();
        let missing: Vec<usize> = witness_vals
            .iter()
            .map(|v| v.inner_ptr() as usize)
            .filter(|p| fed.binary_search(p).is_err())
            .collect();
        if !missing.is_empty() {
            let sample: Vec<String> = missing.iter().take(16).map(|p| format!("{:#x}", p)).collect();
            panic!(
                "E1-FLIP/CEX-1 D5 rendezvous-union oracle FAILED (a): {} live dispatch \
                 handle(s) registered, but {} of their INPUT∪OUTPUT Addr(s) are NOT in the \
                 drained root union fed to the sweep.\n  sample missing inner_ptrs (<=16): \
                 [{}]\n  A registered parallel-dispatch fan-out was not walked — check that \
                 `gc_driver_rendezvous_cycle` calls `collect_live_dispatch_anchors` (D2) and \
                 that `register_live_dispatch` stored the handle in the dispatch handle's \
                 `_live_dispatch` field.",
                live_handles,
                missing.len(),
                sample.join(", "),
            );
        }
    }

    // (b) participant coverage — E1-FLIP Path B V4: the PER-SLOT witness predicate
    // (NOT the fungible parked-count). Re-read `cur_gen` (an independent re-walk, like
    // (a)) and assert EVERY occupied witness slot satisfies the strict-`>` predicate
    // `published>=cur_gen OR acquired>cur_gen` — the SAME live-re-walk the driver wait
    // used. The fungible `workers_parked_for_gc() >= n_snapshot` is NO LONGER the gate
    // (a finisher's bump could "cover" for a parent's not-yet-published machine — the
    // publish-timing UAF the witness fixes); a per-slot violator means a counted-AND-
    // occupied mutator never reified-parked-and-stamped this cycle → under-mark. `n_snapshot`
    // is retained in the message for diagnostics (Pin 1: vestigial on the safety path).
    let cur_gen = ga::current_cycle_gen();
    let (all_ok, violator) = ga::all_occupied_slots_satisfied(cur_gen);
    assert!(
        all_ok,
        "E1-FLIP Path B V4 D5 rendezvous-union oracle FAILED (b): witness slot {:?} is \
         OCCUPIED but NOT stamped for cur_gen {} (published<cur_gen AND acquired<=cur_gen) — \
         a counted-AND-occupied mutator never reached a genuine reified park to \
         `note_reified_park` this cycle, so its (unpublished) machine is NOT in the drained \
         root union (under-mark → the sweep would free a slot a live mutator still holds). \
         This is a per-slot participant-coverage shortfall (n_threads_at_snapshot was {}); \
         the witness gate `current_witness_ok()` should NOT have been set true. The driver's \
         `requestor_wait_for_all_reified_parked` and this oracle use the SAME predicate, so a \
         failure here means a slot was re-occupied for cur_gen AFTER the wait returned and \
         BEFORE this check — a straddle re-park ordering bug.",
        violator,
        cur_gen,
        n_snapshot,
    );
}

/// E1-c (FANOUT>0): trigger a rendezvous collection on the dedicated GC thread
/// (fire-and-forget). Called from a mutator safepoint that observes the watermark
/// while other mutators are live (`n_threads() > 1`). It (a) sets `GC_REQUESTED` so
/// every active mutator parks at its next safepoint, and (b) hands the cycle to the
/// GC thread. The caller then parks at its OWN next safepoint as one of the `n`
/// participants (so the GC thread's `requestor_wait_for_parked_count(n)` completes).
///
/// DORMANT until E1-c step 3 wires the FANOUT>0 safepoint trigger + park path; until
/// then nothing calls this, so the default build is byte-identical.
#[allow(dead_code)] // DEAD until E1-c step 3 wires the FANOUT>0 safepoint trigger.
pub(crate) fn request_concurrent_collection() {
    if !dedicated_gc_enabled() {
        return;
    }
    // Signal every active mutator to poll + park at its next safepoint.
    crate::backend::models::gc_allocator::request_gc();
    // Hand the cycle to the dedicated GC thread (fire-and-forget). If the thread
    // could not be spawned, fall back to clearing the request so mutators do not
    // park forever waiting for a driver that will never run.
    match GLOBAL_GC_DRIVER.get_or_init(spawn_gc_driver).as_ref() {
        Some(driver) => {
            let sent = driver
                .request_tx
                .lock()
                .ok()
                .map(|tx| tx.send(GcDriverRequest::CollectRendezvous).is_ok())
                .unwrap_or(false);
            if !sent {
                crate::backend::models::gc_allocator::resume_workers();
            }
        }
        None => crate::backend::models::gc_allocator::resume_workers(),
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
