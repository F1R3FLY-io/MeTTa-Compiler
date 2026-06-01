# Phase D — D1 (parallel-collector rendezvous) + D2 (worker-self-rooting) design

**Status: DESIGNED (source-verified Plan agent, 2026-06-01), executing. HEAD `54d22a1` (D-TLAB sub-phase
complete).** Lets the index collector run WHILE FANOUT>0 workers are alive — today it is gated OFF the moment
a worker spawns (`!worker_ever_spawned()`), so under parallel eval the heap grows uncollected until
quiescence (the "no-collection-while-parallel" half of the index-PLN perf gap). Authoritative plan:
`~/.claude/plans/help-me-complete-the-shimmying-mochi.md` (Phase D, D1/D2). Companion: `phase-d-design.md`.

## Genuine-CESK crux (the mandate)
The collector CANNOT read a parked worker's registers — they are thread-local raw pointers into that
worker's native stack (`k_spine.rs:81-88` SUSPENDED_ACTIVATIONS/LIVE_VM_STACK). So **D2 = each parked worker
SELF-COLLECTS its own structural roots** (`roots::collect_machine_roots_live` over ITS OWN
operand_stack/work/work_stack/continuations + its `collect_k_spine` thread-locals + `deferred_shared_drops`)
into a shared buffer; the requestor unions them ∪ `collect_persistent_roots(E₀)` ∪ `collect_safepoint_roots`
and marks. Union = `⋃_i σ|_Reachable(⟨C_i,E_i,K_i⟩) ∪ reach(E₀) ∪ reach(driver-C)` — each term read
STRUCTURALLY by its own machine. NO registry, NO Arc root-provider, NO publish-by-value (all A5-deleted). The
shared buffer carries `MettaValue` root HANDLES (what `collect_machine_roots` already produces), not a
serialized machine.

## D1 — the cooperative STW rendezvous
**State (new, `gc_allocator.rs` near `:2806-2817`):** `WORKERS_PARKED_FOR_GC: AtomicU32`,
`GC_REQUESTOR_ACTIVE: AtomicBool` (one collector at a time), `WORKER_ROOT_BUFFER: Mutex<Vec<MettaValue>>`,
NEW `RENDEZVOUS_MUTEX/CONDVAR` + `RESUME_MUTEX/CONDVAR` (do NOT overload the slab `GC_PROGRESS_*`/`QUIESCENT_*`
— cross-wakeup risk). REUSE `GC_REQUESTED`/`request_gc`/`is_gc_requested` for the request signal (one source
of truth; no packed `AtomicU64`). Primary termination gate = **`active_evaluator_count()==0`** (the proven
TLA+ `BeginMark` predicate); `WORKERS_PARKED_FOR_GC` is the buffer-HB carrier + observability aid.

**Requestor** (at a safepoint, when `should_collect*()` AND workers alive): CAS `GC_REQUESTOR_ACTIVE`
false→true (else back off) → `request_gc()` (Release) → self-root (append own machine roots) →
`drop_eval_guard_for_safepoint()` → wait on RENDEZVOUS_CONDVAR until `active==0` (all parked+rooted) →
union ∪ E₀ ∪ driver-C → `mark_sweep_if_over_watermark(union)` under `.write()` (excludes `.read()` allocators
— D-RLOCK) → clear buffer → `GC_REQUESTED.store(false, Release)` → RESUME_CONDVAR.notify_all() →
`GC_REQUESTOR_ACTIVE=false` → `reacquire_eval_guard_after_safepoint()`.

**Worker** (in its closure): (A) WorkerEnter gate at the TOP of `WorkerEvalScope::enter` (`eval_loop.rs:134`):
`while is_gc_requested() { park RESUME }` BEFORE joining the active set (drains `active`, blocks new workers —
the TLA+ `WorkerEnter` `~gcRequested` guard). (B) At each poll point (midloop safepoint `eval_loop.rs:3798`;
JIT `call_support.rs:163`; VM `vm/mod.rs:1331`): `if is_gc_requested() { self_root_into_buffer();
drop_eval_guard_for_safepoint(); WORKERS_PARKED_FOR_GC.fetch_add(1, AcqRel); RENDEZVOUS_CONDVAR.notify_all();
while is_gc_requested() { RESUME_CONDVAR.wait_for(5s) }; reacquire_eval_guard_after_safepoint(); }`.

**Happens-before:** request_gc Release →(HB1) worker is_gc_requested Acquire; worker buffer-append +
`fetch_add(AcqRel)` →(HB2, the AcqRel is the buffer-append release-fence, + RENDEZVOUS_MUTEX held across
predicate+notify) requestor sees ALL buffer writes before mark; `.write()` lock →(HB3) excludes alloc during
mark; `GC_REQUESTED.store(false) Release` →(HB4) worker resume Acquire sees post-sweep. **Lost-wakeup
avoidance** (copied from `EvalGuard::enter` `:2920-2941`): the mutex is held across BOTH the predicate-store
and the `notify_all`, on both the RENDEZVOUS (requestor waits parked-count) and RESUME (worker waits resume)
sides. 5 s `wait_for` + warn-retry (NOT a hard budget — that was the deleted `safepoint_wait_for_quiescence`'s
pathology).

## D2 — worker-self-root handoff
`WORKER_ROOT_BUFFER: Mutex<Vec<MettaValue>>` (single Vec; appends are O(roots/worker), once/worker/cycle).
Each parking worker (poll point) calls `collect_machine_roots_live(&mut my, &machine_operand_stack, &work,
&work_stack, &continuations, env.shared.as_ref())` (the SAME call the midloop safepoint uses at
`eval_loop.rs:3816`, over the worker's own in-scope registers) + each `deferred_shared_drops[i]
.collect_roots_into(&mut my)`, then `WORKER_ROOT_BUFFER.lock().extend(my)`. `collect_machine_roots_live`
folds E₀ — over-counted N× across workers but SOUND (dedup at the `as_arena_addr` mark projection);
**optimization (D2.2): workers append machine-only (no persistent tail), requestor adds
`collect_persistent_roots(E₀)` once.** CESK completeness: because `BeginMark` fires only at `active==0` (every
machine parked-and-rooted or the requestor), no live-but-unrooted machine exists ⇒ mark from the union
retains exactly the reachable set (NoUseAfterFree).

## Sub-increments (each committable + green + DORMANT behind `rendezvous_enabled()` env
`METTATRON_INDEX_GC_PARALLEL=1`, default OFF — byte-identical until D5, like D-TLAB-1.2)
- **D1.1 [FIRST]** — rendezvous state + primitives in `gc_allocator.rs` (`begin_gc_rendezvous`/
  `worker_park_and_root`/`requestor_wait_for_parked`/`end_gc_rendezvous`/`rendezvous_enabled`), ~120 lines
  `pub(crate)`, ZERO call sites (dead code → byte-identical). + a `#[cfg(test)]` 2-thread CAS/condvar test.
- **D1.2 [FIRST, with D1.1]** — `#[cfg(loom)] mod loom_rendezvous` (mirror `index_arena.rs:1840`): 1 requestor
  + 2 workers; assert no-mark-before-all-parked, no-lost-wakeup, no-self-root-after-mark, requestor-exclusion.
- **D2.1** — worker self-root + WorkerEnter gate in `WorkerEvalScope::enter` (`:134`) + the midloop poll point
  (`:3798`), behind `rendezvous_enabled()`. + an index-gc integration test (2 real workers + manual
  requestor) asserting the union covers each worker's roots (CESK completeness; template
  `assert_quiescence_superset` `roots.rs:432`).
- **D2.2** — JIT (`call_support.rs:163`) + VM (`vm/mod.rs:1331`) index poll points (Risk R1: long
  grounded-op/MORK regions). + the E₀-single-count optimization.
- **D2.3** — requestor wiring at the index safepoint (the full §D1 requestor), behind `rendezvous_enabled()`.
  Gate: ASAN-FANOUT>0 + 20-run with `METTATRON_INDEX_GC_PARALLEL=1` (live rendezvous while
  `worker_ever_spawned()`), `cycles_run()>0` + 0 ASAN.
- **D5 (later, irreversible)** — drop `!worker_ever_spawned()` from `gate_open`/`gate_open_midloop`
  (`index_heap.rs:1753/1790`) + default `rendezvous_enabled()` ON. The only non-byte-identical commit.

## Verification
- **loom** (D1.2): the rendezvous protocol (1 requestor + 2 workers, loom condvar/atomics, strong-CAS +
  yield_now, bounded steps).
- **TSan**: the D2.1/D2.3 integration tests under `-Zsanitizer=thread` + `METTATRON_INDEX_GC_PARALLEL=1`.
- **TLA+**: extend `tla/StoreCentricGC.tla` (NOT the deviated `SlabGC_Quiescent.tla`) — it ALREADY has
  `WorkerEnter`-gated-`~gcRequested` (`:288`), `WorkerPark` (`:303`), `BeginMark` requires `activeEvaluators={}`
  (`:416`), QuiescenceInvariant/NoUseAfterFree/NoConcurrentFree. ADD `workerRoots ∈ [Workers → SUBSET Addr]` +
  a `WorkerSelfRoot(w)` action + make `BeginMark` require all parked workers' `workerRoots` populated +
  redefine the mark root set as `psi ∪ ⋃_w workerRoots[w]`; re-prove the three invariants (esp. NoLostObjects
  under the per-worker union). `MC_StoreCentricGC_Rendezvous.cfg` (3 workers, ~6 Addrs).
- **ASAN-FANOUT>0 + 20-run** (D2.3/D5): a worker-spawning PLN workload, `METTATRON_INDEX_GC_PARALLEL=1
  MIN_BYTES=<low>` to fire the rendezvous collector WHILE workers alive; 0 ASAN + `cycles_run()>0` × 20. Cap
  `MemoryMax≤20G MemorySwapMax=0`; treat a spawn-under-load timeout as a flake-retry (once-flake lesson).

## Risk register → proof obligation
| Risk | Fix | Proof |
|---|---|---|
| R1 worker never reaches a safepoint (grounded-op/MORK/JIT) stalls rendezvous | D2.2 JIT/VM poll points + enumerate long regions | 5s wait_for+warn-retry bounds it (not deadlock); TLA+ FairSpec WF(WorkerPark) liveness |
| R2 worker spawns a NEW worker mid-rendezvous | WorkerEnter gate at TOP of `WorkerEvalScope::enter` (before EvalGuard::enter) blocks `~gcRequested` admission | TLA+ WorkerEnter guard; `note_worker_spawned` latches before spawn |
| R3 launder `&'static` held across a parked worker's window | rendezvous mark_sweep DEFERS side-`Box` free (pass `phase != quiescence`, like MIDLOOP) — reclaim node slots only; D4 roots INNER_SHADOW later | conservative (matches midloop gate `index_heap.rs:1985`); ASAN FANOUT>0 |
| R4 trusting deviated condvar primitives | SEPARATE `RENDEZVOUS_*`/`RESUME_*` pairs (not GC_PROGRESS_*/QUIESCENT_*) | loom on the new pairs; extended StoreCentricGC.tla (not SlabGC_Quiescent) |
| R5 RwLock alloc-vs-collect under rendezvous | parked workers hold NO `.read()` (parked between allocs); requestor `.write()` composes w/ D-RLOCK | the tick-balance assert `eval_loop.rs:3531`; ASAN FANOUT>0 |

**Recommended FIRST committable increment: D1.1 + D1.2** (primitives + loom), pure addition + loom-proven,
byte-identical — the foundation D2.1/D2.3 call into.

### Critical files
- `src/backend/models/gc_allocator.rs` (rendezvous state/primitives near `:2806-2817`/`:4097-4146`)
- `src/backend/eval/trampoline/eval_loop.rs` (WorkerEnter gate `:134`; poll point + requestor `:3798-3838`; worker closures `:2449`/`:2956`)
- `src/backend/eval/cesk/roots.rs` (`collect_machine_roots_live` `:373`, `collect_persistent_roots` `:404`, `assert_quiescence_superset` `:432`)
- `src/backend/eval/cesk/index_heap.rs` (`gate_open*` `:1752/:1784`, `mark_sweep_if_over_watermark` `:1902`, the `!worker_ever_spawned()` D5 drops)
- `tla/StoreCentricGC.tla` (extend: `workerRoots`/`WorkerSelfRoot`/union-rooted `BeginMark`; `:288/:303/:416`)
