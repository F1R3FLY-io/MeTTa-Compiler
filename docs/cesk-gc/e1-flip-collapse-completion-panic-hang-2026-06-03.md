# E1-FLIP dominant hang — parallel-collapse completion skips its decrement on panic (TLA+-verified fix)

The **dominant** ~2% robot hang under `FANOUT>0` + the dedicated index collector
(`METTATRON_INDEX_GC_DEDICATED=1`, `--features index-gc`). Distinct from the two GC residuals tracked in
`e1-flip-deadlock-straddle-rootcause-2026-06-03.md` (the rare GC-rendezvous phantom-cycle, Fix B `cb94de8`)
and `e1-flip-collapse-worker-env-gap-2026-06-03.md` (the corruption). Fix B did NOT move the robot hang
needle precisely because the dominant hang is a *different subsystem* — the parallel-collapse completion
handshake, not the GC rendezvous.

## Diagnosis (SIGUSR1 thread-dump — the decisive evidence)
Caught a permanent hang (robot run 129) and dumped thread states via SIGUSR1
(temporary `hang_dump_129_{a,b}.txt` logs): `gc_cycle_in_flight=false`, `gc_requested=false`, `active_evaluators=0`,
and ALL 77 threads `S (sleeping)` on `futex_do_wait` (main + ~56 `work-pool-N` + 4 `mettatron-gc-po` +
`mettatron-index` + `work-pool-overf`), one `mettatron-work-` on `hrtimer_nanosleep` (a Tokio thread,
incidental). **No GC is in flight** ⟹ not a GC-rendezvous deadlock. It is the collapse-completion poll
spinning forever with all workers idle.

## Root cause (source-conclusive)
`parallel_collapse_dispatch` (eval_loop.rs:3356) spawns `N = num_items` workers and a
`remaining = Arc<AtomicU32>` (=N) + `done_pair = Arc<(Mutex<bool>, Condvar)>`. The parent pumps
`cvar.wait_timeout(done, 100µs)` (`pump_parallel_*_wait`, eval_loop.rs:2967 / :3199) and the
`WaitForParallelCollapse` arm (eval_loop.rs:15756) exits ONLY when `remaining = 0` (cancel = `Demand::All`,
never satisfied for plain collapse). The collapse worker (eval_loop.rs:3447) decremented `remaining` at a
**single** site — the closure TAIL (former eval_loop.rs:3612) — and ran `eval_trampoline_with_carrying`
(eval_loop.rs:3555) with **NO `catch_unwind`**. So a PANIC in the eval (a stale swept `Addr` →
`.expect`/`debug_assert` downstream — the corruption bug is the prime panic-FEEDER, but ANY panic does it)
unwinds PAST the decrement; the pool's `catch_unwind` (priority_scheduler.rs:485 + work_pool.rs:1309)
swallows it; the worker goes idle. ⟹ `remaining` is stuck ≥ 1 forever, `done` is never set, the parent
polls forever, all workers idle = the dump. The **branch** worker (`parallel_dispatch`, eval_loop.rs:2594)
already had the `catch_unwind`+decrement-on-panic fix (commit `8fd3a9ac`); the collapse worker's comment
(former :3532) wrongly claimed it didn't need one — exactly the false assumption behind the bug. (A latent
corollary gap existed on the branch path too: a panic in its finisher/lock AFTER the catch and BEFORE the
tail decrement.) Classification: an early-exit (panic) path that skips the completion accounting — NOT a
mis-ordered count, NOT a spawn mismatch, NOT the GC rendezvous.

## Formal verification (TLA+/TLC — `tla/CollapseCompletion.tla`, run before fixing)
Models `remaining`/`done`/per-worker `Finish`-vs-`Panic`/the parent pump, with a `Terminating` stutter so
the **liveness** property `EventuallyDone == <>(parentDone)` is the discriminator. TLC (`/usr/bin/tlc`,
N=3):
- `FixApplied = FALSE` (Panic skips the decrement = the bug): **`Temporal properties were violated`** —
  counterexample is 1 `Finish` + 2 `Panic`s leaving `remaining` stuck at 2, all idle, stuttering forever
  with `parentDone = FALSE`. The dominant hang, formally reproduced.
- `FixApplied = TRUE` (Panic also decrements, via the guard's Drop on the unwind edge): **`No error has
  been found`** — `<>(parentDone)` holds. The fix-condition (the decrement must be reachable on EVERY exit
  path, including unwind) is proven sufficient.

## Fix — `CompletionGuard` RAII (eval_loop.rs:2437; the verified condition realized in code)
A module-local `CompletionGuard { remaining, done_pair }` whose `Drop` (eval_loop.rs:2447) is the SOLE
`remaining.fetch_sub(1, AcqRel)`, sets `*done = true` + `notify_one()` on the `==1` last-out, and recovers
a poisoned `done` mutex (`lock().unwrap_or_else(|e| e.into_inner())` — so a poison can't re-convert into a
hang). Constructed exactly once as the literal FIRST action of each worker closure (before the WorkerEnter
admission wait, `RegionGuard::enter`, and `EvalGuard::enter`), so its `Drop` fires on normal return AND on
panic-unwind (including a re-panicking `resume_unwind`) and on pre-eval admission/setup exits ⟹
exactly-once decrement on every closure-started exit path. The manual
decrements (former collapse :3612, branch :2790 + :2803) were deleted; the branch `catch_unwind`/
`resume_unwind` is kept for cancellation propagation but the count no longer depends on it. Verified: the
only `remaining.fetch_sub` in the file is the guard's; the guard is constructed exactly twice ⟹ no
double-decrement/underflow. Builds clean both backends (49-warning baseline).

### Update 2026-06-04 — closure-top meant literal closure entry

The first post-R-FL bounded Robot smoke reproduced the same all-idle shape on run 5 with the new
`RUN_TIMEOUT` harness: `gc_cycle_in_flight=false`, `gc_requested=false`, `active_evaluators=0`, work-pool
threads asleep, and a partial 223-line output. The source bug was not the guard mechanism; it was placement.
The comments said "closure TOP", but both worker closures still constructed `CompletionGuard` after the
WorkerEnter admission wait, `RegionGuard::enter`, and `EvalGuard::enter`. That leaves pre-guard admission/setup
exits outside the liveness proof.

The guard is now the literal first closure action in both `parallel_dispatch` and
`parallel_collapse_dispatch`. Drop order is also improved: because the guard is declared before `EvalGuard`,
it drops after the active evaluator guard, so the parent is notified only after the worker has left the active
set. Focused validation on 2026-06-04:
- `cargo check --features index-gc --bin mettatron` passed under a 24 GiB cap.
- `cargo build --release --features index-gc --bin mettatron` passed under a 24 GiB cap.
- `RUN_TIMEOUT=180s NORMALIZE_FRESHVARS=1 SORT_OUTPUT=1 scripts/drlock_determinism.sh ... Robot.metta`
  passed 20/20 under `FANOUT=8`, `MTT_GC=index`, `METTATRON_INDEX_GC_DEDICATED=1`,
  `METTATRON_INDEX_GC_MIN_BYTES=131072`; all 20 canonical hashes were
  `82c18bdf4e76ae0b7d66321d38c68144dea24d1e5b124d7e8d3f1bd91870cbb4`.
- TLC fixed config (`CollapseCompletion_fix.cfg`) passed: 28 states generated, 9 distinct, no temporal error.
- TLC bug config (`CollapseCompletion_bug.cfg`) still failed as expected: 55 states generated, 21 distinct;
  counterexample left `remaining = 2`, `parentDone = FALSE`.

## Relationship to the corruption (the deeper root)
The swept-`Addr` corruption (`e1-flip-collapse-worker-env-gap`) is the prime PANIC FEEDER for this hang.
The `CompletionGuard` makes completion **panic-safe** (no hang on ANY panic — defensive, TLA+-verified),
but a panicking worker still yields no result ⟹ the swept-`Addr` panic now manifests as a missing-result
(a wrong-subset corruption) instead of a hang. So fixing the corruption at its source (the swept `Addr`,
via the swept-bitmap arena oracle → root the dominant holder) is the deeper root that removes the panic
entirely (no hang AND no missing result). Both fixes are complementary and both needed.

## Validation
TLA+ verified (above). Robot ×150 (FANOUT=8 DEDICATED=1) on the index-gc binary: expect **HANG → 0** (the
panic-induced strand is now structurally impossible); CORRUPT may persist (the swept-`Addr` residual, the
corruption track). Conformance 483/0 both DEDICATED modes (no regression; the change is in shared
parallel-eval code). Then commit (with `tla/CollapseCompletion.tla`).
