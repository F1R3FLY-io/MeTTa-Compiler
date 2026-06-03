# E1-FLIP GC fix — design iteration log (red-team to convergence)

Tracks the design→red-team→refine loop for the confirmed E1-FLIP bug (see
`e1-flip-VALIDATION-FAILED-2026-06-02.md` §ROOT CAUSE). Goal: a genuine-CESK fix where the dedicated
concurrent collector's root set is complete BY CONSTRUCTION, validated to convergence (a net-subtractive
red-team round) before implementation. NOT YET CONVERGED — do not implement until a round reverses nothing.

## The confirmed bug (fixed target)
Under DEDICATED=1 + FANOUT=8, a collection drops a live atom → wrong PLN subset. Two coupled defects:
(i) the PARENT thread is counted in `n_threads()` but only self-roots at a coarse 4096-iter park, and the
`WaitForParallel` merge moves a live value to the parent's *unpublished* `work_stack` while deregistering
the `LIVE_DISPATCHES` anchor; (ii) the parked-count gate is FUNGIBLE (`workers_parked_for_gc() >= n`,
satisfied by OTHER threads' bumps) → the collector sweeps before the parent publishes.

## Design v1 (Plan agent) — REJECTED by red-team round 1 (NOT net-subtractive; 4 critical flaws)
v1 = Half-B per-thread `PUBLISHED_GEN[]` witness (replace fungible count) + Half-A
`register_temporary_roots(values_of(&merged))` at the dispatch merge + revert ①a to unconditional `true`
with a pump-branch split. Independent red-team (round 1) found it BROKEN:

### Red-team round 1 findings (the v2 spec — each MUST be addressed)
**CRITICAL (each reproduces the bug or a hang):**
1. **`values_of` drops per-result bindings** (types.rs:112-114 = `map(|(v,_)| v)`). PLN threads truth-value
   bindings through `compose_outer_inner_generic` (eval_loop.rs:15332-15343); a value reachable only via a
   binding is NOT in `values_of(&merged)` → swept. **FIX: Half-A must publish the COMPLETE machine via the
   STRUCTURAL reader `collect_complete_thread_contribution(Trampoline{operand_stack, current_work,
   work_stack(incl. the just-pushed Resume), continuations, env0, deferred_envs})` through
   `worker_finish_into_buffer(&roots, my_gen)` — mirroring the EXISTING dispatch finisher
   (eval_loop.rs:2659-2670). NOT `values_of`.**
2. **release-implies-published generation-aliasing** (B1 "on release set PUBLISHED_GEN[i]=cur_gen"): a
   leaving thread reads `current_cycle_gen()` at drop, which is the OLD gen until the driver's END bump →
   either permanent shortfall (HANG/backstop) or, if the slot was reused across a cycle boundary, a stamp
   for the NEW occupant → under-mark (UAF). **FIX: carry `acquired_gen` per slot; a leaving snap-thread must
   go through the FINISHER (publish complete roots), not a release-implies-published shortcut. Stamp only for
   the gen the slot was acquired under, gated `gc_in_progress() && acquired_gen==cur_gen`.**
3. **reverting ①a reopens driver-less producers under dedicated**: `ParallelBranchContext::perform_safepoint`
   `request_gc()` (context.rs:485) + the pump `request_gc()` (eval_loop.rs:2928) fire once
   `parallel_gc_coop_enabled()` is unconditionally true → strand parked workers (no driver). v1 split only the
   pump, not context.rs:479-486. **FIX: KEEP `parallel_gc_coop_enabled() = !dedicated_gc_enabled()` (do NOT
   revert ①a). Wall the NEW publish code behind `#[cfg(index-gc)] && dedicated_gc_enabled()`. Achieve
   DEDICATED=0 byte-identicality via cfg walls, not by re-enabling the legacy coop gate.**
4. **driver waits on the COUNT but the gate checks the WITNESS** → driver's `requestor_wait_for_parked_count`
   returns on `>=n`, then `gate_open_rendezvous`'s witness conjunct is false → `run_collection_if_triggered_
   rendezvous` returns false (no sweep) → `end_rendezvous_cycle`+`resume_workers` run anyway → cycle
   "completes" with NO sweep → unbounded growth → the OOM (rc=137) arm. **FIX: REPLACE
   `requestor_wait_for_parked_count(n)` (gc_driver.rs:182) with `requestor_wait_for_all_published(&snap,
   cur_gen)`; the gate and the wait must reference the IDENTICAL predicate (one source of truth).**

**HIGH (incompleteness — leaks on real workloads):**
5. **Mirror Half-A at the COLLAPSE merge** (eval_loop.rs:15405-15543, the `WaitForParallelCollapse` done-arm,
   handle/`_live_dispatch` drop at ~:15540) — structural twin of dispatch; PLN uses `collapse`/`collapse-bind`.
6. **`WorkItem::Resume` (types.rs:550) has NO field for a root handle** — "ride the handle on Resume" is
   impossible without adding `#[cfg(index-gc)] _merge_root` + defaulting ~30 construction sites + Debug. The
   structural self-root (#1) AVOIDS the handle entirely → prefer #1, drop the handle approach.
7. **Slot lifecycle must be co-located with `N_THREADS` at ALL FOUR transitions**: `EvalGuard::enter`(:3735)/
   `drop`(:3752), `drop_eval_guard_for_safepoint_full`(:5133)/`reacquire_..._full`(:5207), AND the non-full
   `reacquire`(:5158). v1 named only enter/drop → parked workers (leave via `_full`) desync slot vs count.

**MEDIUM (robustness/perf):**
8. **Fixed-cap panicking `PUBLISHED_GEN` array** = a robustness downgrade (work-pool can exceed any cap →
   process panic). Use a GROWABLE/chunked never-realloc directory (the D-TLAB pattern). Gate the slot acquire
   behind `#[cfg(index-gc)] && dedicated_gc_enabled()` so DEDICATED=0/slab stay byte-identical (no
   unconditional free-list CAS on the hot enter path).
9. **The witness is strictly MORE hang-prone than the fungible gate in long non-polling regions** (grounded
   reduction sinks §10 count/sum/hash, MORK `query_multi_act`/type-fixpoint) — it can no longer be unblocked
   by another thread covering. **FIX (liveness): add cooperative `is_gc_requested()` polls inside the long
   grounded sinks + MORK regions, OR a proven bounded-overshoot fallback (a snap-thread stuck in a native
   region whose result is structurally rooted by the D2 dispatch anchor may be treated as published — needs
   its own proof).**

### Red-team synthesis (the converged DIRECTION for v2)
Diagnosis correct; v1's prescription substituted weaker mechanisms. The principled fix = VALIDATION doc
rec #1 (per-thread witness) **AND** rec #2 (the parent must still PUBLISH via the rendezvous): Half-A publishes
the COMPLETE machine (structural reader, BOTH merge arms, via `worker_finish_into_buffer`); Half-B's witness
(a) carries `acquired_gen` per slot, (b) is the SINGLE gate predicate the driver waits on, (c) uses a growable
(not panicking) directory, (d) hooks all four N_THREADS transitions, (e) routes leaving threads through the
finisher; KEEP ①a; cfg+dedicated walls; add liveness polls in non-polling regions.

## Design v2 (Plan agent) — IMPLEMENTABLE SPEC; pending independent red-team round 2
v2 corrected two v1-spec assumptions against source: (Corr-A) the merge arms run in `process_continuation`
(eval_loop.rs:8414) which lacks `operand_stack`/`current_work` — but the tree-walker's operand stack is
ALWAYS empty (eval_loop.rs:3758/3882), so `Trampoline{operand_stack:&empty, current_work:&synthetic_∅,
work_stack, continuations, env0, deferred_envs}` is correct; (Corr-B) ~70% of Half-B already exists
(N_THREADS, GC_CYCLE_GEN, worker_finish_into_buffer/park, drop/reacquire_full, driver, gate) — v2 is a
surgical replacement of the fungible predicate, NOT a from-scratch build.

### Half-A (both merge arms publish the COMPLETE machine; NO values_of, NO Resume handle)
- Dispatch merge (eval_loop.rs ~15356) + Collapse merge (~15540): AFTER pushing `Resume{(merged|result_list,
  env.clone())}` onto work_stack, under `#[cfg(index-gc)] && dedicated_gc_enabled() && is_gc_requested() &&
  eval_guard_depth()>0`: build roots via `collect_complete_thread_contribution(Trampoline{operand_stack:
  &OperandStack::new(), current_work:&Resume{(SmallVec::new(),env.clone())}, work_stack: work_stack.as_slice()
  [now contains the pushed Resume holding merged], continuations: continuations.as_slice(), env0:
  env.shared.as_ref(), deferred_envs: deferred_shared_drops.as_slice(), extra:&[]})` → `worker_finish_into_buffer
  (&roots, current_cycle_gen())`. This roots `merged`+its bindings+the rest of K — the SAME reader the working
  midloop park (4151) uses, the SAME transport the worker finishers (2667/3275) use. `env.clone()` = Arc bump
  (value-identical; RT-9).

### Half-B (per-thread published-gen witness = SINGLE gate predicate)
- Growable never-realloc chunked directory `WitnessChunk{ slots:[WitnessSlot;256], next:AtomicPtr }` near
  N_THREADS (gc_allocator.rs:2788); `WitnessSlot{ acquired_gen:AtomicU64, published_gen:AtomicU64,
  occupied:AtomicBool }` (TWO u64 atomics, NOT packed — RT-3 u32-wrap). Thread-local raw `*const` to its slot.
- Slot lifecycle co-located with N_THREADS at SIX sites (3736/3752/5100/5150/5190/5232), all behind
  `#[cfg(index-gc)] && dedicated_gc_enabled()`. **RT-1 ORDERING: `witness_acquire_slot()` (sets
  acquired_gen=cur_gen) must happen-BEFORE the `N_THREADS.fetch_add` is observable** (both after the
  GC_IN_PROGRESS admission break) so every thread counted in the driver's `n` has its slot in `snap`;
  release-after-Nsub.
- `witness_publish(my_gen)` stamps `published_gen=my_gen` ONLY if `acquired_gen==my_gen`; wired right after the
  existing `note_cycle_bumped(my_gen)` at gc_allocator.rs:3208 (park) + :3250 (finish) — so ALL publish paths
  (Half-A merges, worker finishers, the leave-path zero-root bump 3771) stamp it.
- Driver (gc_driver.rs:177-182): `cur_gen=current_cycle_gen(); snap=snapshot_witness(cur_gen)` (under GIP) →
  REPLACE `requestor_wait_for_parked_count(n)` with `requestor_wait_for_all_published(&snap,cur_gen)` →
  `set_current_witness_ok(true)`. **RT-2 PREDICATE: `all_published` = `∀ occupied slot in snap: published_gen
  >= cur_gen OR acquired_gen > cur_gen`** (>= handles back-to-back-cycle straddle; acquired_gen>cur_gen excludes
  post-snapshot entrants). Gate `gate_open_rendezvous` (index_heap.rs:1783-1784) reads `current_witness_ok()`
  (set by driver post-wait) → gate and wait are ONE predicate (finding #4). Clear the flag in
  `end_rendezvous_cycle`. Upgrade the oracle (gc_driver.rs:253 part-b) to the same witness predicate.
- KEEP ①a unchanged (finding #3). Slab arm keeps `requestor_wait_for_parked_count` (unreachable — slab spawns
  no driver).

### Liveness (finding #9): cooperative `liveness_poll` (publish-and-continue, NO park) — TierLeaf shape
- `liveness_poll(extra)` (gated dedicated+is_gc_requested+depth>0, idempotent via already_bumped_this_cycle):
  `collect_complete_thread_contribution(TierLeaf{extra})` → `worker_finish_into_buffer`. Throttle `&0xFFF`.
- Sites: mork_forms.rs reduction-sink loop :187, per-binding consequent loop :311, iterative join :405-524;
  the MORK `Space::<()>::n` callback in act_tiered.rs/act_persistence.rs. **type_fixpoint runs at depth==0
  (eval/mod.rs:316, OUTSIDE EvalGuard) ⇒ NOT in snap ⇒ NOT a witness-hang source** (v2 corrected the spec here).
- Sound under E1-FLIP because the collector is STW-by-RwLock (sweep holds `.write()`, alloc takes `.read()`):
  a post-publish sink alloc BLOCKS on the write lock until sweep done — no concurrent-alloc UAF (RT-6).

### Edit ordering (each byte-identical at DEDICATED=0; gated cfg+dedicated)
Step 0: witness directory + accessors (dead_code, unit-tested in isolation). Step 1: wire acquire/release at
6 sites (RT-1 ordering) + witness_publish at 3208/3250 (dormant — driver still fungible). Step 2 (THE FLIP):
driver+gate+oracle to the witness predicate. Step 3: Half-A merge publishes (both arms). Step 4: liveness polls.

### Validation gate (Part 7)
(1) corrected discriminator ×16 arms A/B/C all ✅+404+rc0; (2) FANOUT=0 conf 483/0 both backends×DEDICATED;
(3) V4 ASAN 0-UAF + cycles>0; (4) slab nextest + NEW unit test "a finisher bump from a NON-snap thread does
NOT satisfy the witness for a snap-thread"; (5) ×20 determinism; (6) debug oracle 0 panics (upgraded to witness
predicate). Optional TLA+: WitnessComplete + NoPostSnapshotEntrantBlocks.

### v2 self-red-team residuals (resolved): RT-1 acquire-before-Nbump; RT-2 `>=cur_gen` predicate; RT-3 two-u64;
RT-5 benign count-overshoot (count is backstop-only now); RT-6 STW-RwLock blocks post-publish alloc; RT-7 no
new deadlock (condvar releases mutex; same structure as existing wait); RT-8 no slot leak (Drop runs on unwind);
RT-9 env.clone value-identical. **Full v2 in session transcript (agent a7a108799b609869b).**

## Red-team round 2 (independent) — NOT net-subtractive: 1 CRITICAL (CEX-2) + minors → v3 required
v2's Corr-A/Corr-B and the witness mechanism VERIFIED SOUND (operand-stack-always-empty TRUE; scope TRUE;
reader descends into Resume payload + bindings TRUE; RT-1 ordering supported; predicate sound; byte-identical
sound; no deadlock). BUT:

### ★ CRITICAL CEX-2 — the parent's directive RESULT escapes Half-A AND the witness
Trace: merge pushes `Resume{(merged,env)}` (eval_loop.rs:15356) → next iter pops it (8314), if next cont is
`Done` then `*final_result = Some(result)` (8474) — **`final_result` is a Rust local (3731), walked by NO root
source** → loop exits (8331) → returns `results`→`result.0` in eval() (eval/mod.rs:241), still a Rust local →
parent's outer EvalGuard drops (eval/mod.rs:256) taking the **ZERO-ROOT** bump `worker_finish_into_buffer(&[],
my_gen)` (gc_allocator.rs:3771) → witness satisfied with EMPTY publish → the FANOUT>0-gated-off quiescence
collector (index_heap.rs:1752 `!worker_ever_spawned()`=false) never roots `result.0` → dedicated GC thread
drains buffer∪safepoint∪anchors (gc_driver.rs:186-198), NONE contains `result.0` → swept → wrong subset.
**This is the validation symptom.** Half-A doesn't save it (the cycle is usually triggered AFTER the merge, so
`is_gc_requested()` is false at the merge → Half-A no-ops; and the value has moved merge→final_result→result.0
before the trigger). The witness doesn't save it (it guarantees a BUMP, not a COMPLETE publish; the parent's
bump is the zero-root drop bump). CEX-2 is a FIFTH live-Addr class absent from the CEX-1 completeness theorem
(no term for the parent's directive result transiting trampoline-exit → driver-registration).

### The convergent v3 direction (red-team round 2's prescription)
- **CRITICAL: add a DIRECTIVE-EXIT FINISHER** — publish `result.0` + bindings via `worker_finish_into_buffer`
  BEFORE the outer EvalGuard drops, at eval/mod.rs ~241-256 (the eval() exit), the `eval_with_tier` twin, AND
  the batch caller (rholang_integration). Mirror the dispatch finisher (eval_loop.rs:2659-2670).
- **CRITICAL: the EvalGuard-drop ZERO-ROOT bump (gc_allocator.rs:3771) must NEVER satisfy the witness with an
  empty set** — replace it with / supersede it by the directive-exit finisher (which publishes BEFORE the drop).
- **Half-A (merge publishes) is REDUNDANT → DROP it.** The witness FORCES the parent to park at a genuine
  safepoint (branch-B, eval_loop.rs:4124) before the sweep, which reifies `work_stack` (containing the merged
  value). So the merge value is covered by the witness-forced-park, not by a merge-specific publish. (Keep ONLY
  if the completeness sweep finds a window branch-B misses.)
- **Land the directive-exit finisher ATOMICALLY with the witness flip** (Step 2 is NOT independently safe —
  without it, the zero-root bump satisfies the witness and still sweeps the result).
- **MINOR:** move the oracle (gc_driver.rs:253) off the fungible `parked>=n` to a per-slot-published assert;
  pin the intra-slot store/load order (store acquired_gen Release THEN occupied=true Release; driver reads
  occupied Acquire THEN acquired_gen Acquire); replace the enumerated liveness-poll list with a debug tripwire
  ("max iters without a poll while is_gc_requested" assert) per the anti-fragility mandate.

### THE DEEP PRINCIPLE (the genuine-CESK completeness theorem the witness needs)
The witness must be satisfied ONLY by a thread reaching a GENUINE REIFIED SAFEPOINT where its COMPLETE machine
is published — mid-directive: a branch-B park reifying work_stack/continuations; directive-exit: a finisher
publishing result.0+bindings. NEVER by a covering bump (another thread's) NOR an empty/zero-root bump. v3's
completeness obligation: ENUMERATE every point a mutator holds a live value in a native Rust local invisible to
`collect_complete_thread_contribution` at an instant a collection could fire, and show each is either (a) at a
reified safepoint (work_stack/continuations) or (b) covered by a leaving finisher. If bounded → v3 converges.
If whack-a-mole → escalate to a reframe (the collection only proceeds when every counted thread is provably AT
a reified safepoint — classic STW-safepoint discipline) — a USER-GATED decision.

## Design v3 (Plan agent) — completeness sweep ⇒ **BOUNDED**; IMPLEMENTABLE; pending red-team round 3
**State reconciliation:** CEX-1 (D1/D2 + finishers + oracle), the Half-B witness TRANSPORT (GC_CYCLE_GEN,
worker_finish_into_buffer/park, note_cycle_bumped, drop/reacquire_full), ①a/①c, and the Part-6 VM/JIT polls are
ALL already landed. v3 = the witness-predicate FLIP + the directive-exit finisher (atomic) + minors. The driver
still gates on the FUNGIBLE count (gc_driver.rs:182 requestor_wait_for_parked_count; index_heap.rs:1779-1785
workers_parked_for_gc()>=n) — that is what flips.

### Completeness sweep verdict: BOUNDED (linchpin: collect_k_spine reads thread_local SUSPENDED_ACTIVATIONS /
LIVE_VM_STACK ⇒ the GC thread sees a mutator's machine ONLY via that mutator's SELF-PUBLISH at a park/finish).
Every window maps to one of two provenances: (a) reified-machine-at-a-park, or (b) a leaving-result finisher.
- W1.1 parent mid-tramp / W1.2 dispatch merge / W1.3 collapse merge → COVERED by branch-B park (work_stack
  reified); **Half-A redundant → DROP (it was never landed; "drop" = "don't add").**
- **W1.4 directive-exit = THE ESCAPE (CEX-2):** result transits `final_result`(eval_loop.rs:8474)→`results`
  (8362)→`r.0`(mod.rs:241)→EvalGuard drop(mod.rs:256, zero-root bump 3771) with NO safepoint poll and the
  quiescence collector gated off (worker_ever_spawned() sticky-true, gc_allocator.rs:3809) → pervasive (every
  directive after the 1st parallel one). CLOSER = the directive-exit finisher.
- W2.1/W2.2 workers → covered (worker park + dispatch/collapse finisher + D2 anchor). W2.3 cancel/panic → sound
  (value dead, zero-root bump correct). **W2.4 results `try_lock()`-SKIP (types.rs ~298-319) = latent hole**
  (parked worker mid-write → GC thread skips slot) → fix §2.6 (`lock()` under rendezvous); dominance-by-finisher
  UNPROVEN.
- W3.1 VM inter-poll → covered (256-instr poll). **W3.2 JIT long native_fn → SAFETY-covered but LIVENESS risk**
  (poll only at tier-return/pre-eval edges, not instruction-bounded → can stall a cycle; tripwire §2.5).
- W4.* batch → covered (Site A for seq eval + F1 SAFEPOINT_ROOTS handle for workers/consume). W5 type-fixpoint
  depth-0 → not a participant (eval_guard_depth()==0).

### v3 implementation (close W1.4 + flip witness + minors)
- **2.1 Directive-exit finisher (Site A, eval/mod.rs ~239-256):** INSIDE the `_guard` scope, after
  `refresh_thread_local_cache_roots()`, before the block closes (guard still held): under `#[cfg(index-gc)] &&
  dedicated_gc_enabled() && is_gc_requested() && eval_guard_depth()>0`, `collect_complete_thread_contribution(
  TierLeaf{extra:&r.0})` → `worker_finish_into_buffer(&roots, current_cycle_gen())`. **r.0 is the SmallVec<MettaValue>
  directive result; bindings ALREADY collapsed at eval_inner return (mod.rs:966) ⇒ extra=r.0 is COMPLETE, NO
  bindings walk** (simpler than the dispatch finisher). **Site B (tier_forced.rs run_t1/run_jit/run_t0):
  OPEN — required IFF those paths enter an EvalGuard (UNVERIFIED). Site C (batch seq eval) = covered by Site A.**
- **2.2 KEEP the zero-root EvalGuard-drop bump (gc_allocator.rs:3766-3774) — do NOT remove.** For the parent it
  is now SUPERSEDED (the directive-exit finisher publishes+note_cycle_bumped FIRST ⇒ the drop bump short-circuits
  on `!already_bumped`). It stays load-bearing for W2.3 (cancel/panic: value dead, zero roots correct) and **the
  R3 COUPLING: on slot release while `gc_in_progress() && acquired_gen==cur_gen`, it must STAMP
  `published_gen=cur_gen`** (an empty-but-complete publish for a LEAVING thread) — else a thread snapshotted into
  the next cycle between its directive-exit publish (gen K) and its guard drop is waited-for-forever (HANG). THE
  SUBTLEST INVARIANT.
- **2.3 Witness flip (Half-B):** growable chunked `WitnessChunk{slots:[WitnessSlot;256],next:AtomicPtr}`,
  `WitnessSlot{acquired_gen:AtomicU64, published_gen:AtomicU64, occupied:AtomicBool}` near N_THREADS. Acquire at
  depth-0→1 co-located with N_THREADS.fetch_add (RT-1: acquire-store BEFORE the fetch_add is observable — same
  prev==0 block, sites 3735/5190/5232), release at 1→0 (3752/5150). `witness_publish(my_gen)` INSIDE
  `note_cycle_bumped` (2978) — every bump path stamps. Driver (gc_driver.rs:177-182): `snap=snapshot_witness(
  cur_gen)` under GIP → REPLACE `requestor_wait_for_parked_count(n)` with `requestor_wait_for_all_published(&snap,
  cur_gen)`; predicate `∀ occupied s in snap: published_gen>=cur_gen OR acquired_gen>cur_gen` (RT-2 straddle).
  Gate (index_heap.rs:1783) reads driver-set `current_witness_ok()` (ONE predicate, two readers). Oracle
  (gc_driver.rs:290-302) → per-slot-published assert. **Intra-slot order: store acquired_gen Release THEN
  occupied=true Release; read occupied Acquire THEN acquired_gen Acquire.**
- **2.5 minors:** anti-fragility debug tripwire ("max iters w/o poll while is_gc_requested") at the branch-B +
  VM/JIT poll edges (mitigates W3.2). **2.6:** W2.4 `collect_roots_blocking` (`lock()` not `try_lock()`) called
  only from the rendezvous driver.
- **DROP Half-A** (W1.2/W1.3 covered by branch-B).

### Edit ordering (each byte-identical at DEDICATED=0; gated cfg+dedicated)
Step 0 witness directory+accessors (dead_code, unit-tested). Step 1 wire acquire/release at the 4 N_THREADS sites
(RT-1) + witness_publish in note_cycle_bumped (dormant). **Step 2 ATOMIC FLIP: driver+gate+oracle predicate AND
the directive-exit finisher (Site A) + the R3 release-stamp, SAME commit** (neither independently safe: witness-
without-finisher hangs on the parent's slot; finisher-without-witness the zero-root bump still satisfies the
count). Step 3 W2.4 lock(). Step 4 tripwire.

### Validation gate
(1) corrected discriminator ×16 A/B/C all ✅+404+rc0 (score ✅-presence+lines, NOT ❌-count); (2) FANOUT=0 conf
483/0 both backends×DEDICATED; (3) V4 ASAN 0-UAF+cycles>0; (4) slab nextest + unit test "a NON-snap finisher
bump does NOT satisfy a snap-thread's witness"; (5) ×20 determinism; (6) witness oracle 0 panics.

### v3 self-red-team residuals + OPEN ITEMS for round 3
R1 Site A verified; **Site B forced-tier-guard UNVERIFIED (blocking)**. R2 keep-zero-root-bump sound. **R3 the
release-stamp-on-leaving coupling = THE subtlest invariant (independent re-derivation needed).** R4 JIT liveness
(W3.2) bounded-but-not-liveness-complete. R5 W2.4 dominance unproven. **The STW-safepoint REFRAME is NOT needed
for safety** (witness-forces-park ≈ STW already); it's the clean answer to W2.4+W3.2 → user-gated Phase-F option.
**Full v3 in transcript (agent a4da4be19e9acf019).**

## Red-team round 3 (independent, FOCUSED) — NEEDS-V4: 3 NEW criticals (2 classes the "BOUNDED" sweep MISSED)
- **Item 1 (Site B):** forced-tier `run_t0/t1/jit` enter NO EvalGuard (guard-less) ⇒ Site B unneeded. BUT
  **(1-a)** forced-tier+DEDICATED=1 is then UNCOVERED (guard-less parent never self-roots work_stack) → document
  forced-tier+DEDICATED OUT-OF-SCOPE; validation runs Auto only. **(1-b) CRITICAL — NEW E₀ CLASS:** Site A's
  `TierLeaf{extra:&r.0}` OMITS `reach(E₀)`. If a cycle's participants all FINISHED (none parked mid-trampoline
  with a `Trampoline` contribution that walks env0), E₀ (named spaces/global bindings/types/rule_index values) is
  unrooted → swept → UAF next directive. Fix: Site A must publish `collect_persistent_roots(env0=result.1.shared)
  ∪ r.0` (mirror the inline hook eval/mod.rs:297-301), NOT TierLeaf.
- **Item 2 (R3 release-stamp): CRITICAL — NOT SOUND.** Gen-conditional (`acquired_gen==cur_gen`) → no-ops exactly
  in the K/K+1 straddle it targets → HANG. Gen-unconditional → stamps "published" while `result.0` is still a
  live local in the window main.rs:256→790 (NO mark-sweep root there; GcHoldGuard gates session-release only) →
  K+1 UAF on result.0. **Fix: route the directive result through PERSISTENT SAFEPOINT_ROOTS (cross-cycle, drained
  every cycle via collect_safepoint_roots gc_driver.rs:187), exactly like the batch F1 finisher — a
  SafepointRootHandle rooted inside eval()'s guard scope, rode out to main.rs, dropped AFTER consume.** Then the
  release-stamp's empty publish is genuinely sound (result.0 no longer depends on the per-cycle buffer). This
  supersedes Site A's TierLeaf AND the WORKER_ROOT_BUFFER release-stamp for the parent.
- **Item 3 (W2.4): DOMINATED (not a real escape) → DROP §2.6.** The finisher publish (eval_loop.rs:2667) precedes
  the slot store (2674) ⇒ slot ⊆ buffer; AND the parked-count gate serializes the anchor walk AFTER all writes
  (a mid-write worker hasn't parked ⇒ wait hasn't returned). `lock()` adds a GC-thread deadlock edge for zero
  gain → KEEP `try_lock()`.
- **Item 4 (W3.2 JIT liveness): CRITICAL for the default-flip.** SAFETY OK (witness waits; nothing live swept).
  But JIT polls are ARG-EDGE-bounded NOT instruction-bounded ⇒ a long native_fn loop → UNBOUNDED global stall in
  RELEASE (the debug tripwire is release-INERT) → the OOM/hang arm. Fix: in-JIT back-edge `is_gc_requested()` poll
  (Option B, principled) OR rendezvous-backoff while any thread is in native JIT (Option C, `JIT_NATIVE_DEPTH`
  counter, stopgap). Tripwire stays debug-only forgot-an-edge check, NOT the liveness guarantee.

**The trend is NOT converging:** rt1=4, rt2=1, rt3=3 NEW criticals; the "BOUNDED" sweep MISSED the E₀ (1-b) and
cross-cycle-result (2) classes. The per-window-finisher approach is revealing itself as ENUMERATE-Y — the exact
"remember to register every root" fragility the migration exists to ELIMINATE (guaranteed-by-construction is the
STANDING requirement). 

## ⛔ DESIGN FORK (USER-GATED) — surfaced 2026-06-02; do NOT implement either path without user direction
- **Path A — continue bounded-patch (v4):** the rt3 spec (SAFEPOINT_ROOTS-route the directive result + E₀ rooting
  at Site A + JIT poll/backoff + drop §2.6 + re-sweep). Implementable, but ENUMERATE-Y (a pile of per-window
  finishers); 3 rounds keep finding new classes; rt3 itself wants ANOTHER completeness sweep ⇒ low confidence it
  converges in 1 more round; VIOLATES guaranteed-by-construction.
- **Path B — the REIFIED-PARK-ONLY witness (≈ STW-at-reified-safepoints), RECOMMENDED:** the witness slot is
  stamped ONLY by a GENUINE reified safepoint-park (full machine + E₀), NEVER by a TierLeaf finisher or a
  zero-root/leaving bump; every counted thread MUST reach a reified park before the sweep (directive-exit parks
  with the result still on work_stack OR routes it to SAFEPOINT_ROOTS as a leaving-park; JIT/long-ops get bounded
  polls). GUARANTEED-BY-CONSTRUCTION (no native-code window can escape — the sweep only runs when all are
  reified). Builds on the validated witness direction (a refinement, not from-scratch). COST: pause-time (all
  park) + bounded-polls-everywhere; the user CHOSE hybrid-reify-at-safepoint to avoid hot-path regression, so the
  pause/poll cost needs their sign-off. This is the genuine-CESK answer their architecture-requirement implies.
- **Recommendation:** Path B. The 3-round evidence shows bounded-patching re-introduces the enumerate-roots
  fragility; Path B delivers the guaranteed-by-construction completeness the migration's core motivation demands,
  and is a refinement of the witness work already validated. Awaiting user direction before implementing.

## ✅ RESOLUTION (2026-06-02): user chose **PATH B** (after briefly choosing A then reconsidering). "Done right" + "red-team it!"
Path B design COMPLETE + recorded → **`docs/cesk-gc/e1-flip-pathB-design.md`** (the implementation spec). Key
outcomes: SAFETY is **guaranteed-by-construction** (closure at the GATE — the sweep can't run unless the witness
predicate holds, satisfiable only by a reified park publishing the complete machine); the 2 classes the v3 sweep
MISSED (E₀, cross-cycle result) are closed structurally (B2′ driver-walked live-env registry + B3 persistent
SAFEPOINT_ROOTS result channel). Three premise corrections: **B2 (E₀-as-one-global-Arc) INFEASIBLE → B2′**
(live-env registry, register at every EvalGuard::enter + branch-spawn, granularity (a)); **B3 ride-to-caller**
(not held-in-eval-scope); **liveness is a fail-safe residual** (missed poll = HANG not UAF) → recommend a single
`NATIVE_OP_DEPTH` RAII backoff. ▶ NEXT = INDEPENDENT red-team on Path B → iterate to net-subtractive → implement
(Steps 0-4, dormant/gated) → validation gate → commit dormant → flip reserved for user.

**Resources:** 8 agents (3 design + 3 red-team + root-cause + initial), ~1.7M subagent tokens. Production SAFE
throughout (default DEDICATED=0, fix UNCOMMITTED, HEAD 8070c78). Full rt3 in transcript (agent a5adb42437273269a).
