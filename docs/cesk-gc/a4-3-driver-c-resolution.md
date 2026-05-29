# A4.3 — driver-C resolution (MettaState.source gap)

The A4.3 oracle FIRED and caught a real gap (`|OLD|=38 |NEW|=33 |OLD−NEW|=5`): the 5
missing roots are `MettaState.source` — the program's top-level directives. This is the
DRIVER's control (the eval/conformance/rholang loop evaluating each directive), held
ABOVE the trampoline, NOT in the machine's ⟨C,E,K⟩. `eval_trampoline_inner` receives
`ctx: &C` (an `EvalContext`), not a `MettaState`. Exhaustive check: `MettaStateGcRoots`
is the ONLY `collect_all_roots()` registry provider `collect_machine_roots` doesn't
already cover (env→collect_structural; 5 OnceLock + 4 TL caches→collect_global_anchors;
current-iter = the C work item; the 2 parallel providers excluded by the single-threaded
oracle gate).

## Resolution: Option B (the seam) + A (the oracle algebra)
Source-verified: the `ctx` reaching the firing midloop safepoint IS a `SessionContext`
(the public `arena_engine::eval_trampoline` does `SessionContext::new(state)`), and
`SessionContext` holds `state: &MettaState` + exposes `state()`. So the driver↔machine
boundary already carries the program. Read it there.

1. **`MettaState::collect_driver_program_roots(&self, out)`** (metta_state.rs) — reads
   `gc_roots.source` + `gc_roots.output` by name (the exact bodies `MettaStateGcRoots::
   collect_roots` uses; lock order source-before-output, uncontended at the safepoint per
   the ABBA-fix). One source of truth for the driver-C roots.
2. **`EvalContext::collect_driver_roots(&self, _out)`** (context.rs) — defaulted no-op
   (contexts with no driver program — `StaticEvalContext`, `ParallelBranchContext` — keep
   it).
3. **`SessionContext::collect_driver_roots`** override (session_context.rs) →
   `self.state.collect_driver_program_roots(out)`. The real body; the driver↔machine seam.
4. **Oracle reformulation** (eval_loop.rs): assert `OLD ⊆ (NEW ∪ KEPT)` where
   KEPT = `ctx.collect_driver_roots`. One filter line:
   `new.binary_search(p).is_err() && driver_c.binary_search(p).is_err()`. Panic gains a
   4th class (d): a driver-C root not exposed via `ctx.collect_driver_roots`.

```
OLD  = root_set.roots() ∪ collect_all_roots()                     (unchanged)
NEW  = collect_machine_roots(S,C,work,K,E₀-env) ∪ deferred-drop append   (unchanged)
KEPT = ctx.collect_driver_roots = SessionContext → MettaState.{source,output}   (NEW)
ASSERT  OLD ⊆ (NEW ∪ KEPT)   (sorted-deduped inner_ptr multiset)
```

## Why NON-VACUOUS
KEPT is the two NAMED Vecs `source`+`output`, NOT the bundled `collect_all_roots()`. So
NEW alone must still cover S∪C∪K, frame_chain/k-spine, the 9 caches, E₀, and the
deferred transient — a missing cache / k-spine guard / transient still fails the oracle.
KEPT subtracts EXACTLY the genuinely-kept driver program. (Residual: KEPT can only mask a
root literally in source/output; a same-`inner_ptr` alias on the non-moving arena ⇒ same
live object ⇒ not actually dropped ⇒ not a soundness hole.)

## A4.4 / A5 forward-link
- **A4.4 midloop flip:** `midloop_roots = collect_machine_roots(...) ∪ deferred-drop ∪
  ctx.collect_driver_roots(...)` — the driver-C becomes a REAL collector input via the
  same seam.
- **A4.4 quiescence flips** (eval/mod.rs:266, tier_forced.rs:285): there `state: &MettaState`
  is a direct param, so append `state.collect_driver_program_roots(out)` directly (no ctx
  seam). (Reconciles the prior plan's `roots.extend(result)`: result ≠ source/output;
  those come from collect_all_roots today and must be re-added explicitly post-flip.)
- **A5.3 (delete ROOT_REGISTRY):** when `MettaStateGcRoots`'s provider impl is deleted,
  source+output survive via `collect_driver_program_roots` fed at the flip sites. No
  separate METTA_STATE_REGISTRY needed.
- **A5.4 (narrow SAFEPOINT_ROOTS to driver-C):** `collect_driver_program_roots` becomes
  the canonical driver-C payload through the narrowed `register_temporary_roots` buffer;
  the oracle's KEPT then reads the buffer (same set, narrower mechanism). Ordering hazard:
  move KEPT's source to the buffer in the SAME commit that deletes the provider.

## Green-wall
- RELEASE (oracle + new bodies cfg'd out of hot path; the trait default + override +
  inherent method are uncalled in release ⇒ dead-code-eliminated or `#[allow(dead_code)]`):
  slab 4325 / index 4177 / conf 483·221·40 byte-identical, 0 new warnings.
- DEBUG index (oracle fires): the CI test `a4_3_oracle_holds_across_safepoint` ((cnt 5000 0)
  + amb) now GREEN — the 5 source directives land in KEPT, the machine roots in NEW.
