# Path B — REIFIED-PARK-ONLY witness: implementation spec (guaranteed-by-construction)

User-chosen (final), "done right". The guaranteed-by-construction fix for the E1-FLIP concurrency bug
(see `e1-flip-VALIDATION-FAILED-2026-06-02.md` + `e1-flip-FIX-design-iterations.md` for the full history:
v1/v2/v3 bounded-patch all rejected; rt1=4, rt2=1, rt3=3 criticals; the bounded approach IS the
enumerate-roots fragility the migration exists to kill). Pending an independent red-team before implementing.

## Principle (one sentence)
The dedicated GC sweep proceeds ONLY when the witness directory proves every counted slot was stamped THIS
cycle by a stamp carrying a COMPLETE machine — and "complete" is guaranteed because (B1) only a *reified park*
or the *directive-exit leaving-park* may stamp (never a TierLeaf-finisher / zero-root / other-thread bump);
(B2′) E₀ is walked unconditionally by the GC thread from a global live-env registry (no participant
dependence); (B3) the directive result rides SAFEPOINT_ROOTS out of eval(); (B4) every unbounded native
region has a bounded poll / backoff so every thread can reach a park; (B5) the gate is the per-slot witness
predicate replacing the fungible count. **Closure is at the GATE, not per-window.**

## Three source-verified corrections to the original Path B premises
- **C-0a — B2 "E₀ as ONE process-global Arc" is INFEASIBLE.** `atom_space`(core.rs:324)+`rule_index`(core.rs:409)
  ARE global Arcs, but `named_spaces/bindings/types/subtypes/states`(core.rs:331-362) are per-
  `GenericEnvironmentShared` RwLock fields CoW-cloned at fork (core.rs:842-921); `STATIC_ENV`(context.rs:174)
  is thread_local. No global handle to the env struct (roots.rs:410-412 confirms). ⟹ use **B2′** (global
  registry of live env-`shared` Arcs).
- **C-0b — "a co-existing Trampoline park supplies E₀" does NOT hold by construction** (trigger thread may
  finish-via-TierLeaf + all workers finish ⟹ no Trampoline participant ⟹ E₀'s env struct unrooted = rt3 ITEM
  1-b). ⟹ B2′ is load-bearing, not optional.
- **C-0c — B3 must RIDE the result handle to the caller** (F1 pattern); held-in-eval()-scope leaves a sliver
  `[eval() returns, main.rs:790 re-register]`.

## B1 — witness stamped ONLY by a genuine reified park (separate BUMP=liveness from STAMP=safety)
- Keep the fungible parked-count bump (`WORKERS_PARKED_FOR_GC`, gc_allocator.rs:3204/3246) as a pure condvar
  liveness-wake — unchanged.
- Add `note_reified_park(gen)` (NEW) — stamps the thread's `WitnessSlot.published_gen=gen` iff acquired_gen==gen.
  Called ONLY from the 3 reified-park sites: (1) midloop branch-B park `worker_park_and_root_in_cycle`
  (eval_loop.rs:4195, a `Trampoline` contribution); (2) `worker_cooperative_safepoint` index park
  (eval_loop.rs:238, TierLeaf — complete BECAUSE B2′ makes E₀ global); (3) the B3 directive-exit leaving-park.
- The zero-root EvalGuard-drop bump (gc_allocator.rs:3768-3773) keeps `note_cycle_bumped` (liveness) but NEVER
  `note_reified_park` ⟹ it can never satisfy the witness (closes rt3 ITEM 2's empty-publish-satisfies BY
  CONSTRUCTION). Worker finishers (eval_loop.rs:2667/3275, TierLeaf) publish to WORKER_ROOT_BUFFER for
  completeness but do NOT stamp — a finished worker RELEASED its slot (N_THREADS--), so it's not in `snap`;
  its result is covered by the D2 dispatch anchor (results[slot]) + the parent's K-frame.

## B2′ — E₀ env struct as a GC-walked global live-env registry (mirrors SAFEPOINT_ROOTS/LIVE_DISPATCHES)
- NEW in gc_allocator.rs (~:4636): `trait EnvRoots{collect_env_roots}` + `static LIVE_ENVS:
  OnceLock<Mutex<Vec<Option<Weak<dyn EnvRoots>>>>>` + `LiveEnvHandle`(RAII Drop frees slot) +
  `register_live_env(&Arc<dyn EnvRoots>)->LiveEnvHandle` + `collect_live_env_anchors(&mut Vec)`.
- `impl EnvRoots for GenericEnvironmentShared` body = the EXISTING `collect_roots_into` (core.rs:2170) — zero
  new traversal.
- Register (RAII) at **every EvalGuard::enter (eval/mod.rs:240) AND every branch-worker spawn
  (eval_loop.rs:2586)** — granularity (a), recommended (fully participant-independent; covers CoW-forked
  child bindings which live in a DIFFERENT shared Arc, core.rs:916). Driver walks it every cycle:
  `collect_live_env_anchors` at gc_driver.rs:187 (beside collect_safepoint_roots). Weak ⟹ self-healing.
- ⟹ E₀ in the root set EVERY cycle regardless of which participants parked (rt3 ITEM 1-b closed at the driver).

## B3 — directive-exit = SAFEPOINT_ROOTS leaving-park (the verified F1 pattern, rholang_integration.rs:557-628)
- Inside eval()'s `_guard` scope (eval/mod.rs ~253, before drop): `register_temporary_roots(r.0)` →
  SAFEPOINT_ROOTS (drained every cycle, gc_driver.rs:187), RIDE the `SafepointRootHandle` to the caller
  (change eval() return to carry `Option<SafepointRootHandle>` under cfg; callers main.rs:730/745/1046 +
  eval_with_tier twin drop it AFTER register_temporary_roots at :790/:1058) — closes the sliver (C-0c).
- ALSO `note_reified_park(current_cycle_gen())` if is_gc_requested() — the empty-machine leaving-stamp, SOUND
  because E₀ is in B2′ + r.0 is in the persistent channel + the thread is leaving. Gen-unconditional-safe (no
  per-cycle-buffer dependency) ⟹ closes rt3 ITEM 2's cross-cycle UAF AND the K/K+1 hang BY CONSTRUCTION.

## B4 — liveness: every counted thread can reach a stamp (fail-safe residual; recommend NATIVE_OP_DEPTH backoff)
Covered already: trampoline (branch-B 0xFFF), workers (branch-B / finish-release), VM (256-instr poll →
coop-safepoint). NEEDS handling: **JIT native_fn** (arena.rs:161/344, unbounded, no back-edge poll) + **MORK/
grounded sinks** (mork_forms.rs:194/225/311/405/475/496/524 + ProductZipper match_conjunction_query_multi:377;
possibly fileio/string/state grounded ops). **Recommend: a single `NATIVE_OP_DEPTH: AtomicU32` (RAII guard,
panic-safe) incremented at EVERY boundary into unbounded non-trampoline native code; the driver's witness-wait
backs off while `NATIVE_OP_DEPTH>0`.** Soundness: such a thread is structurally rooted (VmLeaf::Jit K-leaf
arena.rs:152 + B2′), so DEFERRING the sweep until it exits loses nothing; a MISSED increment fails SAFE (the
K-spine still protects it). This coarsens the enumeration to one-line-per-boundary; type-fixpoint runs at
depth-0 (eval/mod.rs:320, OUTSIDE EvalGuard) ⟹ not a participant.

## B5 — the witness predicate (the gate)
- Directory (NET-NEW): `WitnessSlot{acquired_gen:AtomicU64, published_gen:AtomicU64, occupied:AtomicBool}` +
  `WitnessChunk{slots:[_;256], next:AtomicPtr}` near N_THREADS (gc_allocator.rs:2788), growable never-realloc.
  TWO u64 (not packed, rt2 RT-3). Intra-slot order: write acquired_gen Release → occupied Release; read
  occupied Acquire → acquired_gen Acquire → published_gen Acquire (rt3 minor + A7 ABA).
- Acquire/release at the 5 N_THREADS sites (gc_allocator.rs:3735/3752/5150/5190/5232), cfg+dedicated gated.
  **RT-1: acquire-store BEFORE N_THREADS.fetch_add is observable** (same prev==0 block, ordered first) so every
  thread counted in `n` is in `snap`. reacquire_full RE-STAMPS acquired_gen=current_cycle_gen() (resumed worker
  rejoins under the current gen).
- Driver flip (gc_driver.rs:177-204): `snap=snapshot_witness(cur_gen)` (under GcInProgressGuard, after admission
  closed) → REPLACE `requestor_wait_for_parked_count(n)` with `requestor_wait_for_all_reified_parked(&snap,
  cur_gen)`. Predicate: `∀ slot∈snap: published_gen>=cur_gen OR acquired_gen>cur_gen` (rt2 RT-2 straddle) AND
  `NATIVE_OP_DEPTH==0` (B4). 5s warn-recheck backstop. Then `set_current_witness_ok(true)`.
- Gate `gate_open_rendezvous` (index_heap.rs:1783) reads `current_witness_ok()` — ONE predicate, two readers
  (rt1 #4). Clear in `end_rendezvous_cycle` (gc_allocator.rs:3416). Oracle (gc_driver.rs:253/293) → per-slot
  assert (NOT the fungible count).

## Guaranteed-by-construction proof (sketch) — SAFETY SOLID
Sweep runs ⟹ `current_witness_ok()` ⟹ `∀ slot∈snap: published_gen>=cur_gen ∨ acquired_gen>cur_gen` AND
NATIVE_OP_DEPTH==0. `snap` taken after GcInProgressGuard closed admission (gc_allocator.rs:3708 gates
EvalGuard::enter). Partition threads holding a live Addr:
- **S1 counted+occupied:** predicate forces published_gen>=cur_gen ⟹ stamped by a reified park (B1) ⟹ machine
  published to WORKER_ROOT_BUFFER BEFORE the stamp, and the thread is now BLOCKED on RESUME_CONDVAR (can't
  advance past the park till post-sweep gen bump) ⟹ live values ⊆ R.
- **S2 finished+released** (slot∉snap): result in results[slot] ⟹ D2 anchor (gc_driver.rs:197) + parent K-frame.
- **S3 post-snapshot entrant** (acquired_gen>cur_gen): excluded; can't pass EvalGuard::enter under GC_IN_PROGRESS
  (gc_allocator.rs:3708-3712); .write()-gated sweep ⟹ allocate-black.
- **S4 in native op** (NATIVE_OP_DEPTH>0): sweep does NOT run (backoff); structurally rooted via K-leaf + B2′.
- **E₀:** in R via collect_live_env_anchors (B2′) every cycle. **driver-C/result:** in R via SAFEPOINT_ROOTS (B3
  + main.rs:790 + collect_driver_program_roots).
Every state ⊆ R ⟹ sweep reclaims only dead. ∎  A NEW native window added later is auto-safe (holds EvalGuard ⟹
counted ⟹ must publish-then-stamp or the gate never opens; a forgotten poll ⟹ HANG, not UAF).

## Self-red-team residuals (A1-A8) — for the independent round to verify
- **A1:** B2′ at granularity (a) (register at branch-spawns too) is REQUIRED for CoW-forked child bindings.
- **A2:** B3 MUST ride-to-caller (signature change, 4 sites) — held-in-scope leaves a sliver.
- **A3 (WEAKEST):** the unbounded-native-region enumeration for liveness is not exhaustive (fileio/string/state
  grounded ops, ProductZipper) ⟹ use the `NATIVE_OP_DEPTH` backoff (coarse, fail-safe).
- **A4:** worker-TierLeaf-OK holds BECAUSE of B2′ (not without it).
- **A5:** no new deadlock (distinct condvars; NATIVE_OP_DEPTH thread doesn't wait on the driver) — PROVIDED
  native_fn terminates + the counter is RAII (A6).
- **A6:** NATIVE_OP_DEPTH MUST be RAII (panic-safe) — a bare fetch_add/sub leaks on JIT panic ⟹ permanent backoff.
- **A7:** witness slot ABA handled by the straddle predicate + intra-slot Release/Acquire order.
- **A8:** pause cost = safepoint-stride (the design the user chose) + a long native op delays a cycle until it
  exits (bounded by the op, not the stride) — the residual the user signs off on.

## Honest assessment
SAFETY: **SOLID by construction** (conditional on B2′ at granularity (a), B3 ride-to-caller, NATIVE_OP_DEPTH
RAII, atomic landing of witness-flip + B3 + B2′). LIVENESS: a **bounded, fail-safe residual** (missed poll =
hang surfaced by the oracle + 5s warn-loop, never corruption). B2 infeasibility (→ B2′) is the one premise
correction. This is the genuine-CESK answer; the two classes the v3 sweep MISSED (E₀, cross-cycle result) are
closed BY CONSTRUCTION (driver-walked E₀ registry + persistent result channel), not by another finisher.

## Edit ordering (each byte-identical at DEDICATED=0; cfg+dedicated walls)
Step 0 (dead_code): witness directory + accessors + B2′ LIVE_ENVS/EnvRoots + NATIVE_OP_DEPTH RAII; unit test
"non-reified bump does NOT satisfy". Step 1 (dormant wiring): acquire/release at 5 N_THREADS sites (RT-1) +
note_reified_park at the 3 sites + register_live_env at enter+spawns + NATIVE_OP_DEPTH guards. Step 2 (ATOMIC
FLIP): driver+gate+oracle→witness predicate + collect_live_env_anchors + B3 ride-to-caller (witness-flip + B3 +
B2′ together — neither independently safe). Step 3: B4 NATIVE_OP_DEPTH backoff conjunct + MORK polls. Step 4:
debug tripwire.

## Validation gate
(1) corrected discriminator ×16 A/B/C all ✅+404+rc0 (✅-presence+lines, NOT ❌-count); (2) FANOUT=0 conf 483/0
both backends×DEDICATED; (3) V4 ASAN 0-UAF+cycles>0; (4) slab nextest + "non-reified-bump does NOT satisfy"
unit test; (5) ×20 determinism; (6) per-slot oracle 0 panics. **Full design in transcript (agent adbd144d3f03d90f4).**

---

# Path B — RED-TEAM ROUND 1 (independent) → NEEDS-REFINEMENT → **v2** (below)
Critical 4(a) + 2 more criticals + mediums/minors. The architecture (closure-at-the-gate) is RIGHT; the
in-native-code treatment was wrong.

## ★ CRITICAL 4(a) — the NATIVE_OP_DEPTH backoff is UNSAFE (UAF, not hang)
The in-native (JIT `native_fn` arena.rs:161; MORK query; long grounded sink) register file lives in
`thread_local!` `LIVE_VM_STACK`/`SUSPENDED_ACTIVATIONS` (k_spine.rs:84-87); `collect_k_spine` reads the
CALLING thread's copy; the GC thread's root union is ONLY 3 GLOBAL sources (drain WORKER_ROOT_BUFFER ∪
collect_safepoint_roots ∪ collect_live_dispatch_anchors, gc_driver.rs:186-198) — it NEVER calls collect_k_spine.
`roots.rs:441-444` states this as an invariant ("the GC thread physically cannot reach thread B's caches — B
MUST self-publish"). ⟹ an in-native thread is INVISIBLE to the GC thread until it self-publishes at a park.
With the NATIVE_OP_DEPTH==0 backoff, a MISSED increment lets the sweep proceed while the thread mutates its
invisible live register file → swept → **UAF** (not the claimed hang). The enumeration of native regions is
non-exhaustive (A3), so "missed increment = UAF" is a live safety hole. The red-team's "global VM/JIT anchor"
suggestion is INSUFFICIENT (it gives visibility but the live register file is still being MUTATED → torn-read
race; you need the immutable snapshot a park produces).

## THE v2 FIX (cleaner + genuinely by-construction): DROP the backoff; the WITNESS is the SOLE gate
- **Remove NATIVE_OP_DEPTH entirely** (and the global-anchor idea — unnecessary). `current_witness_ok()` ⟺
  every counted slot stamped THIS cycle. No escape hatch.
- An in-native counted thread MUST stamp to let the sweep run; it can stamp ONLY by reaching a reified park
  (worker_cooperative_safepoint → snapshot its register file into WORKER_ROOT_BUFFER → note_reified_park). It
  CANNOT stamp without parking. ⟹ the sweep runs ⟺ ALL counted threads are PARKED (machines snapshotted,
  immutable) ⟹ the GC marks only immutable snapshots ⟹ NO race, NO missed live value. **Guaranteed-by-construction.**
- A thread stuck in an unbounded native region never stamps ⟹ the sweep WAITS (hang, loud via the oracle + 5s
  warn-recheck) — it NEVER proceeds-and-races. **Fail-safe by construction.** This makes the original "missed
  poll = hang not UAF" claim actually TRUE (it was false only because the backoff provided a proceed-without-
  stamp escape).
- **In-op polls (Option B) are now LIVENESS-ONLY** (so a thread CAN reach a park within bounded time): MORK
  sinks (mork_forms.rs:194/311/405) + grounded long ops get a throttled `if is_gc_requested(){
  worker_cooperative_safepoint(&inflight)}`. A missing one = HANG (debuggable), not corruption.
- **JIT native_fn (arena.rs:161):** VERIFY boundedness — if each native_fn is a bounded chunk that returns to
  the trampoline per reduction (so the trampoline branch-B 0xFFF poll covers it), NO in-JIT poll is needed; if
  a native_fn can run an unbounded compiled loop, EITHER add an in-JIT back-edge poll OR (clean stopgap)
  **gate the JIT OFF under DEDICATED=1** (JIT-on-index is opt-in perf; disabling it eliminates the unbounded-
  native-region class entirely). v2 must determine which.

## The other red-team findings to fold into v2
- **CRITICAL slot-lifecycle (target 5):** the witness slot op must co-locate with N_THREADS at ALL SIX sites
  including the NON-full `drop_eval_guard_for_safepoint`/`reacquire` pair (gc_allocator.rs:5100/5190) — the v3
  RT-7 finding. A slot released via :5100 without a witness release ⟹ phantom-occupied ⟹ HANG. **Co-locate
  slot-op + N_THREADS in ONE helper so they cannot diverge (by-construction).**
- **CRITICAL ordering (target 5, ITEM-2 invariant):** B3 must `register_temporary_roots(r.0)` → (Release fence)
  → `note_reified_park` → drop guard. Pin it; a stamp-before-register window = the ITEM-2 UAF.
- **MEDIUM B2′ deadlock (target 2):** the GC thread's `.read()` walk of registered env Arcs (collect_roots_into,
  core.rs:2174-2266 takes ~7 RwLock reads) DEADLOCKS if a worker is parked holding an env-struct `.write()`
  (e.g. mid add_to_space). VERIFY no park site is reachable while holding an env write lock; else `try_read()`
  + a completeness argument.
- **MEDIUM backstop (target 8b):** the witness wait's 5s backstop MUST warn-and-recheck ONLY, NEVER
  proceed-on-timeout (with the witness-sole-gate there is no other escape; a proceed-on-timeout = silent UAF).
- **MINOR callers (target 3):** eval()/eval_with_tier have DOZENS of callers (lib.rs, both conformance bins,
  rholang_integration) — a missed one is a COMPILE error (tuple-arity), not a UAF, but bind the handle to a
  NAMED `_root_handle` local (not `_`) at every site + build the lib.rs test surface in the gate.
- **SOUND, keep:** the witness mechanism (two-u64 slot, straddle predicate, RT-1, S3 exclusion); B2′ at
  granularity (a) with the verified-complete collect_roots_into body; B3's F1 ride-to-caller; S2 finisher
  triple-coverage; the STW-by-RwLock allocate-black.

## Path B v2 = v1 with: backoff DROPPED (witness sole gate) + six-site co-located slot helper + fenced B3 order
+ B2′ deadlock check + warn-recheck backstop + JIT boundedness decision + named-local handles. ▶ v2 design +
re-red-team next.
