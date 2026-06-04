# CESK GC migration — A4 re-alignment + A5 path (Plan-agent corrected, 2026-05-30)

HEAD `390b743` (A4.4). This corrects a DRIFT: chasing the midloop oracle's caught gap, the
last (uncommitted) change threaded `*const MettaState` through the bytecode VM — "patching
root-discovery into machine internals," the anti-CESK pattern that derailed the prior session.
**That VM-state plumbing was REVERTED to `390b743`** (drift saved recoverably at
the temporary `vm_plumbing_drift_from_390b743.patch` artifact). The committed A4.1–A4.4 ARE genuine CESK and stand.

## STANDING ANTI-DRIFT GUARDRAIL (read before every GC change)
A change is **CESK migration** iff it: (a) reifies more machine state into the readable registers
⟨C, E_local, K⟩ / E₀; (b) makes a root set read STRUCTURALLY from those registers (+ the ONE narrow
driver→GC publication channel for the driver's own C); or (c) DELETES / cfg-scopes a piece of the
discovery apparatus (ROOT_REGISTRY, RootProvider, register_root_provider, collect_all_roots*,
frame_chain, SAFEPOINT_ROOTS-as-provider, per-context driver-C seams, Arc root-providers,
drop-worker/INNER_SHADOW/publish-by-value rooting).
A change is **revising the existing GC** (FORBIDDEN — the drift) iff it ADDS or PATCHES a
root-collection channel, threads roots through machine internals (e.g. a `*const MettaState` on the
VM, a new per-context collector method), or enumerates live values the machine should already expose.
**Acid test:** grows the number of *places* that contribute roots ⇒ DRIFT. Grows what the *one*
structural reader walks (or routes a driver root onto the *one* existing narrow channel) ⇒ MIGRATION.
**When the oracle flags a missing root**, the CESK response is the diagnostic question — "Is this root
structurally in ⟨C,E,K⟩, in E₀, or in the ONE narrow driver-C channel? If not, why is the machine not
reified enough — or is this legitimately the driver's meta-level C (→ the single narrow publication
channel)?" — NEVER "add another collector."

## VERDICT — `MettaState.source` is a DRIVER root, not a machine root
The per-directive CESK machine's ⟨C,E,K⟩ is one directive's redex/env/continuation. The residual
top-level directive sequence is consumed by a meta-level DRIVER loop (`for expr in state.source()`
in conformance_common.rs:179 / main.rs / REPL) ABOVE the machine. It is the driver's control C, NOT
a machine register. Correct treatment = the plan's **A5.4: the driver publishes its program to the
ONE narrow SAFEPOINT_ROOTS channel** (`register_temporary_roots`, the same channel the conformance
`all` accumulator at mtt_conformance.rs:189 and the REPL `filtered_results` at main.rs:775/1033
already use), read uniformly at every collection site via `collect_safepoint_roots`. NO VM plumbing.
NO per-context seam. Reifying the program INTO the machine would make a different (whole-program)
machine — out of scope, rejected.

## Disposition of the committed driver-C mechanisms
- `collect_safepoint_roots()`-as-KEPT (A4.4, committed): ON-PLAN (it IS the narrow channel). Keep.
- `MettaState::collect_driver_program_roots` body (A4.3, committed): KEEP — the publication payload.
- `EvalContext::collect_driver_roots` per-context seam (A4.3, committed): REDUNDANT 2nd driver-C
  mechanism. A5.4 SUBSUMES it: once the driver publishes source/output to SAFEPOINT_ROOTS, the
  midloop's `ctx.collect_driver_roots` + the quiescence `state.collect_driver_program_roots` calls
  become redundant (covered by `collect_safepoint_roots` already called there) → delete the trait
  method + SessionContext override; oracle KEPT simplifies to `collect_safepoint_roots` alone.

## A5 sub-steps (each green-walls BOTH builds before commit; ASAN where flagged) — see a5-deletion-plan.md
- A5.0 cfg seams + `push_expr_vec_frame` helper (no deletion).
- A5.1 index GC roots via typed K-spine ONLY; frame_chain spine/VM pushes cfg→slab. (+ASAN index)
- A5.2 re-home module-import/assertion ExprVec to the K-spine; delete `maybe_push_frame`. (+ASAN index)
- A5.3 cfg-wall the 10 RootProvider impls + registrations to slab (keep inherent bodies). (+ASAN BOTH)
- A5.4 narrow SAFEPOINT_ROOTS to driver-publication transport **+ fold in driver-C** (driver publishes
  source/output to SAFEPOINT_ROOTS; delete the `EvalContext::collect_driver_roots` seam; simplify
  oracle KEPT). This resolves the midloop driver-C gap STRUCTURALLY (the midloop already calls
  `collect_safepoint_roots`). (+ASAN slab)
- A5.5 cfg-wall registry CORE to slab — INDEX BUILD BECOMES REGISTRY-FREE (genuine-CESK milestone);
  drop the index oracle's `collect_all_roots()` OLD-term (stays non-vacuous on live root_set). (+ASAN BOTH, 20-run)
- A5.6 `frame_chain.rs` cfg→slab-only (index has no frame_chain module). (+ASAN index)
- A5.7 graduate both oracles to permanent CI invariants; full green-wall + ASAN both + 20-run +
  mmverify + HE-bisim 40/40 + PLN budgets. A5 COMPLETE.

Then B (concurrent arena/TLAB/JIT-on-index) → C (generational + abstract-GC marking) → D (parallel
collector; cross-thread apparatus dissolves via per-worker structural reads) → E (concurrent SATB on
E₀ + selective-CESK* choice-point unification + serializable continuations) → F (Welch + index-default
+ F4 physically delete the bridge/slab/apparatus; final `rg` of every apparatus symbol = 0).
