# Path B v2 — implementation spec (reified-park-only witness, witness-SOLE-gate)

The verified, implementable design. Supersedes v1's NATIVE_OP_DEPTH backoff (dropped — it was a UAF
escape-hatch: the in-native register file is thread_local-invisible to the GC thread, k_spine.rs:84-87 +
roots.rs:441-444). Witness-SOLE-gate ⟹ "missed poll = HANG not UAF" by construction. Pending a FOCUSED
independent red-team on A-straddle-2 (below) before implementation. Full design: transcript agent a2a15b4fefbec9275.

## Source-verified facts (line-precise)
- 6 N_THREADS sites CONFIRMED: enter add gc_allocator.rs:3736; drop sub :3752; non-full drop sub :5100; full
  drop sub :5150; non-full reacquire add :5190; full reacquire add :5232.
- `worker_park_and_root_in_cycle` (gc_allocator.rs:3197) is called from EXACTLY the 2 reified parks
  (eval_loop.rs:4195 branch-B `Trampoline`-incl-env0, :238 coop-safepoint `TierLeaf`) + tests. ⟹ put the stamp
  INSIDE it (:3208, after note_cycle_bumped, under RENDEZVOUS_MUTEX + gen-gate). `worker_finish_into_buffer`
  (:3239, the finishers + zero-root drop bump) gets NO stamp ⟹ can never satisfy the witness BY CONSTRUCTION.
- **JIT native_fn is BOUNDED** (recursion → JitBailoutReason::TailCall call_support.rs:740 → polled VM
  vm/mod.rs:1715; intra-chunk jumps statically bounded). ⟹ NO in-JIT poll, NO JIT-off. (A long grounded op
  inside the JIT fast-path is the only residual native region — safety-covered by the witness-hang, liveness-
  covered by Step-3 grounded polls.)
- **B2′ deadlock unreachable**: no named_spaces/bindings/types/states `.write()` scope spans an eval/safepoint
  (verified add_rule rule_management.rs:3238 is a pure insert; heuristic scan empty). Use `try_read()` in
  collect_env_roots as defense (a skip ⟹ covered by the writing mutator's own park).
- Flip targets CONFIRMED: driver wait `requestor_wait_for_parked_count(n)` gc_driver.rs:182; gate
  `workers_parked_for_gc()>=n_threads_at_snapshot()` index_heap.rs:1782-1784; oracle gc_driver.rs:292-302.
- ALL witness/B2′ symbols are NET-NEW (zero grep). ~70% of the transport already exists dormant.

## Step 0 — witness directory + B2′ registry (dead_code, gated, byte-identical)
gc_allocator.rs (near note_cycle_bumped:2980): `WitnessSlot{acquired_gen:AtomicU64, published_gen:AtomicU64,
occupied:AtomicBool}`, `WitnessChunk{slots:[_;256], next:AtomicPtr}` (grow-only never-realloc), thread_local
`MY_WITNESS_SLOT:*const`, `CURRENT_WITNESS_OK:AtomicBool`. TWO u64 (rt2 RT-3). Intra-slot order: write
acquired Release → published Release → occupied=true Release LAST; read occupied Acq → acquired Acq →
published Acq. Fns: `witness_acquire_slot`(acquired=current_cycle_gen, published=acquired-1, occupied=true),
`witness_release_slot`(occupied=false), `witness_restamp_acquired(g)`, `note_reified_park(g)`(iff occupied &&
acquired==g: published=g — THE SOLE STAMP), `snapshot_witness`(→ stable slot POINTER list, never-realloc),
`requestor_wait_for_all_reified_parked(&snap,cur_gen)`(LIVE-RE-WALK each wake — see A-straddle-2; predicate
`∀ slot: published>=cur_gen OR acquired>cur_gen`; 5s warn-recheck, NEVER proceed-on-timeout),
`set/current_witness_ok`. B2′: clone LIVE_DISPATCHES pattern (gc_allocator.rs:4753) → `trait EnvRoots`,
`LIVE_ENVS`, `register_live_env`→`LiveEnvHandle`(RAII), `collect_live_env_anchors`; `impl EnvRoots for
GenericEnvironmentShared{ collect_roots_into }` (core.rs:2170, complete-verified), `try_read()`.

## Step 1 — SIX-site CO-LOCATED slot helper + the stamp (dormant, byte-identical)
A single helper co-locates witness-op with N_THREADS at all 6 sites so they CANNOT diverge (the :5100 omission
was the v3-RT-7 phantom-slot HANG): enter/reacquire(prev==0) → `witness_acquire_slot()` BEFORE the fetch_add
(RT-1); drop/safepoint-drop/full-drop → `witness_release_slot()` AFTER the fetch_sub; both reacquires re-stamp
acquired=current_cycle_gen. Stamp: `note_reified_park(my_gen)` INSIDE worker_park_and_root_in_cycle:3208. All
`#[cfg(index-gc)] && dedicated_gc_enabled()`.

## Step 2 — THE ATOMIC FLIP (witness sole gate + B2′ + B3) — ONE commit, none independently safe
- (2a) Driver gc_driver.rs:177-204: keep n+set_n_threads_at_snapshot (oracle); `cur_gen=current_cycle_gen()`
  under _gip; REPLACE requestor_wait_for_parked_count(n) (:182) with `requestor_wait_for_all_reified_parked(
  &snap,cur_gen)`; then `set_current_witness_ok(true)`; add `collect_live_env_anchors(&mut roots)` at :188.
- (2b) Gate index_heap.rs:1782-1784 → `current_witness_ok()`. (2c) end_rendezvous_cycle:3416 →
  `set_current_witness_ok(false)`. (2d) Oracle :292 → per-slot assert (SAME live-re-walk predicate).
- (2e) B3 directive-exit (eval/mod.rs:239-256, inside _guard, gated dedicated&&is_gc_requested): ORDER =
  `let leaving=collect_persistent_roots(result.1.shared) ∪ result.0` (NOT TierLeaf — include E₀) →
  `let _root_handle=register_temporary_roots(leaving)` → `fence(Release)` → `note_reified_park(current_cycle_gen())`.
  CHANGE eval() return to carry `Option<SafepointRootHandle>` (#[cfg(index-gc)]); RIDE to caller, drop AFTER
  consume. Callers (bind NAMED `_root_handle`): main.rs:730/745/1046 + :733/:748 eval_with_tier; both
  conformance bins; rholang_integration; ~25 lib.rs sites (gate must `cargo test --lib`). forced-tier+DEDICATED
  OUT-OF-SCOPE (rt3 1-a, guard-less; validation Auto only). register_live_env (B2′) RAII at eval/mod.rs:240 +
  branch-spawn eval_loop.rs:2710 (granularity (a), covers CoW-forked child bindings core.rs:916).

## ★ A-straddle (SUBTLETY 1b) — the re-park + live-re-walk (THE crux; A-straddle-2 = the residual to red-team)
A thread T parked for cycle K (snapshotted into K's WORKER_ROOT_BUFFER, which is CLEARED at end_rendezvous_cycle
:3420) wakes, finds cycle K+1 already collecting, blocks at reacquire admission (gc_allocator.rs:5214). T's
machine (work_stack on its paused Rust stack) is NOT in K+1's buffer and NOT globally walked (thread_local
registers) ⟹ K+1 would sweep it → **UAF on resume**. FIX = the **re-park-on-new-cycle** handshake in
`reacquire_eval_guard_after_safepoint_full` (:5207): loop { if gc_in_progress() && gen!=my_reparked_gen:
witness_acquire_slot(); worker_park_and_root_in_cycle(&reparked_roots, gen) [re-publishes machine + stamps gen
+ waits gen-end]; witness_release_slot(); my_reparked_gen=gen; continue. else if !gc_in_progress(): rejoin
(ACTIVE+=depth, witness_acquire_slot, N_THREADS+=1, set depth). else: block on condvar }. Requires threading
`reparked_roots:&[MettaValue]` into the reacquire signature (2 call sites: eval_loop.rs:239 pass &my_roots,
:4201 pass &park_roots — my_roots lives in the trampoline frame across the park). Slot occupied EXACTLY while
re-published-and-parked for gen g ⟺ T's machine in g's buffer (atomic under RENDEZVOUS_MUTEX). Induction over
intervening cycles ⟹ no cycle sweeps T's unpublished machine. ∎
**A-straddle-2 (the residual to red-team):** the driver's wait must NOT use a value-snapshot (it would miss a
slot T re-occupies AFTER the snapshot instant → sweep-without-waiting → UAF). FIX = `snapshot_witness` returns
stable slot POINTERS (never-realloc) and `requestor_wait_for_all_reified_parked` LIVE-RE-WALKS the grow-only
chunk list from WITNESS_HEAD each wake (picks up a slot re-occupied-for-cur_gen + newly-grown chunks). O(slots)
per wake, cheap. The oracle uses the SAME live-re-walk. THIS is the subtlest invariant (replaces v3's
release-stamp coupling) — the focused red-team must re-derive it.

## Step 3 — MORK/grounded liveness polls (LIVENESS-ONLY; miss = HANG, fail-safe)
Throttled (`&0xFFF`) `if is_gc_requested(){worker_cooperative_safepoint(&inflight)}` at mork_forms.rs:194/201/
225/311/405/414/475/496/524 + long grounded ops. VM (vm/mod.rs:1350) + JIT (call_support.rs:194) ALREADY wired.

## Guaranteed-by-construction proof (safety)
Sweep runs ⟺ current_witness_ok ⟺ (P)`∀ slot∈snap: published>=cur_gen OR acquired>cur_gen`. note_reified_park
(the only published-setter) fires only inside worker_park_and_root_in_cycle (the 2 reified parks + the re-park)
+ B3 (after register E₀∪result.0 to SAFEPOINT_ROOTS). Partition: S1 counted+occupied → forced-stamped → parked
(immutable snapshot in buffer ⊆ R) or B3-leaving (E₀∪result.0 ⊆ SAFEPOINT_ROOTS ⊆ R); S2 finished+released →
D2 anchor + parent K-frame; S3 post-snapshot entrant (acquired>cur_gen) → excluded + admission-blocked; S4
in-native → must reach a park to stamp ⟹ sweep WAITS (hang, fail-safe, no race); E₀ → B2′ every cycle;
results → SAFEPOINT_ROOTS. Straddle → re-park (above). Every state ⊆ R ⟹ reclaim only dead. ∎ Liveness residual
= the Step-3 poll enumeration (miss = HANG, debuggable via oracle + 5s warn-loop), NOT a UAF.

## Byte-identical DEDICATED=0 / edit ordering / gate
NO unconditional hot-path atomic (no NATIVE_OP_DEPTH). All witness/B2′/B3 ops `#[cfg(index-gc)] &&
dedicated_gc_enabled()` (cached OnceLock); slab const-folds out; B3 body gated dedicated&&is_gc_requested (=
never under DEDICATED=0 since request_concurrent_collection early-returns gc_driver.rs:316); eval() Option field
#[cfg(index-gc)] (slab sig unchanged). Order: Step0 (dead) → Step1 (dormant, co-located) → Step2 (ATOMIC FLIP
incl. straddle re-park + live-re-walk) → Step3 (liveness) → Step4 (debug tripwire). GATE: (1) discriminator ×16
A/B/C ✅+404+rc0 (✅-presence, NOT ❌-count); (2) FANOUT=0 conf 483/0 both backends×DEDICATED; (3) V4 ASAN 0-UAF
+ cycles>0; (4) slab nextest + 3 unit tests (non-reified-bump-rejected, non-snap-bump-rejected, STRADDLE
park@K→re-park@K+1); (5) ×20; (6) per-slot oracle 0 panics; (7) `cargo test --lib` both backends (catches a
missed B3 caller — compile error).

## ⚠️ v2 STRADDLE was BROKEN (CRITICAL UAF, attack #3) → **v3 = Fix-3A** (below). Everything else SOUND.
The focused red-team found v2's re-park loop releases the slot BETWEEN consecutive re-parks → a back-to-back
cycle K+2 snapshots T's slot in the seam (occupied=false, machine in no buffer, T not in n_threads) → sweeps
T's frozen machine → UAF. Attacks #1/#2/#4/#5/#6 + the witness-sole-gate proof + stamp-reachability + B2′/B3 +
byte-identical = ALL SOUND. So v3 is a LOCALIZED straddle-loop fix, not a redesign.

### v3 = v2 with Fix-3A (continuous slot occupancy across the straddle)
In `reacquire_eval_guard_after_safepoint_full` (gc_allocator.rs:5207), the re-park loop:
```
witness_acquire_slot();                 // ONCE, BEFORE the loop (occupied=true, acquired=my_gen=K)
let mut my_reparked_gen = my_gen;        // =K (the gen T originally parked for)
loop {
    let g = current_cycle_gen();
    if gc_in_progress() && g != my_reparked_gen {
        witness_restamp_acquired(g);     // acquired=g — SLOT STAYS OCCUPIED (never released here)
        worker_park_and_root_in_cycle(&reparked_roots, g);  // re-publish machine + note_reified_park(g) + wait g-end
        my_reparked_gen = g;
        continue;
    } else if !gc_in_progress() {
        break;                           // rejoin
    } else { /* g==my_reparked_gen, that cycle still draining: block on RESUME/GC_PROGRESS condvar */ }
}
// rejoin: ACTIVE += saved_depth; N_THREADS += 1; set depth; witness_release_slot();  // release ONCE, AFTER rejoin
```
T's slot is occupied for the ENTIRE straddle ⟹ a K+2 snapshot in any seam reads occupied=true,
published=K+1<K+2, acquired∈{K+1 or K+2}≤K+2 ⟹ predicate FALSE ⟹ driver WAITS ⟹ no gap, fail-safe. The
`reparked_roots: &[MettaValue]` is threaded into reacquire_full's signature (2 call sites pass &my_roots /
&park_roots; the borrow spans the loop; T runs nothing during the straddle so the snapshot stays complete;
correctness depends on the NON-MOVING collector).

### v3 pins (from the red-team; all must hold)
- **STRICT `>` predicate:** `published>=cur_gen OR acquired>cur_gen` — NEVER `acquired>=cur_gen` (a
  re-stamped-but-not-yet-republished slot `acquired==cur_gen, published<cur_gen` MUST read "wait"; the torn-read
  combinations are all safe ONLY with strict `>`).
- **N_THREADS asymmetry is INTENTIONAL (document in the helper):** the straddle re-park keeps the slot occupied
  WITHOUT an N_THREADS++. The witness is the safety gate; n_threads/parked-count is a liveness backstop that
  tolerates T's undercount (n) + parked-overshoot. Do NOT "fix" the Step-1 co-location helper to bump N_THREADS
  at the re-park (that corrupts the next cycle's n).
- **CONDVAR constraint:** the driver's `requestor_wait_for_all_reified_parked` MUST wait on `RENDEZVOUS_CONDVAR`
  holding `RENDEZVOUS_MUTEX` across {predicate, wait_for}; the stamp MUST be inside worker_park_and_root_in_cycle's
  existing gen-gated RENDEZVOUS_MUTEX block (3208). Then the parker's notify (3205) is never lost (attack #1).
- **NON-MOVING pin:** re-publishing the same reparked_roots Addrs across cycles is correct ONLY because the
  collector is non-moving (a surviving Addr stays at its slot). If ever made moving, this breaks.
- **MINOR liveness:** optionally notify RENDEZVOUS_CONDVAR on witness_acquire_slot (avoid a 5s stall on
  new-chunk growth; not safety — new entrants are admission-blocked at 3708).

### v3 gate additions (v2's single-re-park straddle test is INSUFFICIENT)
- **Back-to-back straddle unit test:** T parks@K; end K; BEFORE T re-acquires, fire K+1 AND K+2 in succession;
  assert T's slot stays occupied across both seams + each sweep's union contains T's reparked_roots Addrs.
- **ASAN back-to-back straddle (V4 variant):** the above under -Zsanitizer=address, ≥2 intervening cycles per
  straddle, 0 UAF.

## ⚠️⚠️ v3/Fix-3A was ALSO insufficient (the BASE protocol releases the slot across the park) → **V4 = decouple slot-occupancy from N_THREADS**
The final red-team found the ROOT (which the prior rounds, incl. Fix-3A's proposer, missed): the Step-1 6-site
helper RELEASES the witness slot at the safepoint full-drop (:5150) and only re-acquires at reacquire (:5232) —
so a PARKED thread is UNOCCUPIED during its park, excluded from `snap` (which collects only occupied slots),
exactly when it holds a frozen unpublished machine. `note_reified_park(K)` at :3208 then NO-OPs (slot not
occupied) ⟹ the witness-sole-gate proof's S1 ("counted+occupied→forced-stamped→parked") is VACUOUS (no parked
thread is ever counted-AND-occupied). This re-opens the empirically-confirmed publish-timing window
(VALIDATION-FAILED §ROOT-CAUSE: "the sweep must not begin until EVERY thread counted in the n-snapshot has
ACTUALLY published"). The 6-site helper CONFLATED two DISTINCT lifecycles.

### V4 — the slot lifecycle (the genuine guaranteed-by-construction invariant)
**INVARIANT: a witness slot is OCCUPIED ⟺ the thread holds an unpublished-this-cycle LIVE machine — from
`EvalGuard::enter` continuously to the thread's OUTERMOST drop, INCLUDING across every park.** DECOUPLE it from
N_THREADS (which keeps its own lifecycle: released at park, a parked thread isn't "active").
- `EvalGuard::enter` (prev==0): `witness_acquire_slot()` (acquired=cur_gen) BEFORE `N_THREADS.fetch_add` (RT-1).
  Slot OCCUPIED.
- safepoint `drop_eval_guard_for_safepoint`(:5100)/`_full`(:5150): `N_THREADS.fetch_sub` — but **do NOT
  `witness_release_slot()`.** Slot STAYS OCCUPIED (the frozen machine is still live).
- `worker_park_and_root_in_cycle` (:3203 publish → :3208 `note_reified_park(my_gen)`): now lands (occupied==true
  && acquired==my_gen) ⟹ T ∈ snap with published>=cur_gen ⟹ the driver correctly waited-then-proceeds, marking
  T's machine from the buffer. S1 is now NON-VACUOUS.
- `reacquire_..._full` rejoin (:5232) + the straddle re-park: `witness_restamp_acquired(cur_gen)` +
  `N_THREADS.fetch_add` — **NO release.** Slot stays OCCUPIED; T resumes running WITNESSED (the next cycle waits
  for it until it parks again — steady-state behavior).
- straddle re-park loop: with the slot held from enter, the pre-loop acquire is unnecessary; each intervening
  cycle does `witness_restamp_acquired(g)` → `worker_park_and_root_in_cycle(&reparked_roots, g)` (re-publish +
  stamp g). The slot is occupied throughout ⟹ the driver waits for T at EVERY cycle until T re-stamps for it ⟹
  NO seam (closes attacks #1/#3/#4 by construction).
- true outermost `EvalGuard::drop` (:3752): `witness_release_slot()` AFTER `N_THREADS.fetch_sub`. ONLY here does
  the slot become unoccupied.
**Keep:** the strict-`>` predicate (`published>=cur_gen OR acquired>cur_gen`); the RENDEZVOUS_CONDVAR+MUTEX wait;
the live-re-walk; the non-moving pin; the intentional N_THREADS-vs-slot asymmetry (now FULLY decoupled, not just
at the re-park). The Step-1 helper must have SEPARATE slot-op and N_THREADS-op (NOT co-located one-to-one):
slot acquire@enter / release@outermost-drop / restamp@reacquire+rejoin; N_THREADS add@enter+reacquire /
sub@every-drop.

### V4 straddle trace (guaranteed-by-construction)
T parks K: occupied(acq=K), publish→K-buffer, stamp pub=K. Cycle-K snap: pub(K)>=K ⟹ satisfied ⟹ mark K-buffer
(has T) ⟹ safe. K ends: gen→K+1, buffer cleared, T wakes (gen≠K). Cycle-K+1 snap (T not yet re-parked):
occupied, pub=K. pub(K)>=K+1? no; acq(K)>K+1? no ⟹ driver WAITS for T. T (slot still occupied) re-parks:
restamp acq=K+1, publish→K+1-buffer, stamp pub=K+1, notify. Driver re-reads: pub(K+1)>=K+1 ⟹ satisfied ⟹ mark
K+1-buffer (has T) ⟹ safe. ∎ No window where T is occupied-but-unwaited or unoccupied-but-live.

## ✅✅ V4 CONVERGED — final independent red-team (agent ab8de1cb711c1872d): NET-SUBTRACTIVE, all 6 attacks SOUND, no UAF.
The reified-park-only witness, witness-SOLE-gate, strict-`>` straddle predicate, live-re-walk over a
never-realloc pointer directory, slot-held-enter-to-outermost-drop = the genuine guaranteed-by-construction fix.
**Confirmed final slot-lifecycle (the Step-1 helper MUST separate slot-op from N_THREADS-op, NOT co-locate
one-to-one):** slot ACQUIRE only at EvalGuard::enter prev==0 (:3735) BEFORE the fetch_add; RELEASE only at
EvalGuard::drop depth==1 (:3752) AFTER the fetch_sub; RESTAMP (acquired=current_cycle_gen, no release) at both
reacquires (:5190/:5232) + each straddle re-park; slot UNTOUCHED at BOTH safepoint drops (:5100/:5150) + the
dead non-full reacquire; STAMP (note_reified_park) inside worker_park_and_root_in_cycle:3208 (gen-gated) + B3 at
eval/mod.rs:255 after register+fence.

### 3 NON-BLOCKING implementer pins (wiring, not design holes)
1. `n`/`set_n_threads_at_snapshot` is VESTIGIAL on the safety path post-flip (oracle 2d + gate 2b use the witness)
   — keep only as a liveness pre-wait or delete; document (harmless if left computed-but-unread).
2. B3 leaving-stamp soundness DEPENDS on SAFEPOINT_ROOTS being gen-unconditional (drained EVERY cycle,
   gc_driver.rs:187) — do NOT optimize it into a per-cycle buffer, or the B3 K/K+1 window re-opens.
3. The Step-1 helper's safepoint-drop arm must skip the slot op for the non-full `:5088`/`:5100` path identically
   to the full `:5150` path (the v3-RT-7 phantom-slot class; closed by "no slot op at ANY safepoint drop").

### Secondary residuals (accepted): re-park livelock under alloc-storm (throughput, not safety); long-grounded-op
liveness (witness-hang-safe, Step-3 polls; miss = HANG not UAF); forced-tier+DEDICATED out-of-scope (Auto only).

## ▶▶ IMPLEMENT (Steps 0-4, dormant/gated, byte-identical DEDICATED=0) → validation gate → commit dormant → flip reserved for user.
