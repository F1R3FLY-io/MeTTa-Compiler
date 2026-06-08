# Single-Threaded Store-Centric Garbage Collector (Increment 6 — first WORKING collector)

Status: **IMPLEMENTED, validating** (2026-05-28). Gated behind `--features index-gc`.
Default (slab) build is byte-identical.

## Goal

Wire the already-correct, currently test-only mark/sweep core
(`IndexHeap::mark(&[Addr])` + `IndexHeap::sweep()`) as a **live collector** for
the **provably-single-threaded** evaluation regime, so it is safe *by
construction* — no concurrent-collector hazards, no admission-gate/rendezvous.
The parallel-rendezvous collector is a separate, later increment and is
explicitly OUT of scope here.

A TLA+ proof (`tla/StoreCentricGC.tla`, all 5 safety invariants exhaustively
verified) establishes that mark+sweep AT QUIESCENCE is safe. The single-threaded
regime makes "quiescence" trivially hold: there is exactly one evaluator and no
parked-resumable worker, so the single calling thread at a between-steps
safepoint is the SOLE thread that can touch the store σ.

## STEP 0 — The UAF-critical linchpin: how the collector sees the live S/C/K

This is the question that decides correctness. A mark-sweep collector that marks
from an INCOMPLETE root set frees live values → use-after-free.

### How the SLAB collector obtains the sequential trampoline's S/C/K

The sequential trampoline (`eval_loop.rs`, the `eval_internal` loop) runs a
periodic safepoint every 4096 iterations (`gc_counter & 0xFFF == 0`,
`eval_loop.rs:3378`). Inside that block it builds a `RootSet<MettaValue>`
(`src/backend/eval/cesk/roots.rs`) named `root_set` containing the COMPLETE live
trampoline state (`eval_loop.rs:3386`–3415):

```
root_set.clear();
root_set.collect_from_work_items(&work, &work_stack);      // C: focus + work stack
root_set.collect_from_continuations(&continuations);        // K: continuation stack
collect_frame_chain_roots(root_set.as_mut_vec());           // caller frame chain
collect_eval_memo_roots / collect_match_result_roots(...)   // pointer-keyed caches
tabling::collect_subgoal_roots / thunk::collect_thunk_roots(...)
for deferred_env in &deferred_shared_drops { ...collect_roots(...) }  // deferred env drops
```

Collapse-bind capture frames are not a separate root source in the current
collector: they carry metadata (`tracked_vars` and fork depth), while the
value-bearing bindings travel with `BoundValue` and are already walked through
the work item / continuation root readers.

It then passes that root vec **directly** to the collector via
`ctx.perform_safepoint(root_set.drain_into_vec())` (`eval_loop.rs:3462`).

For the SEQUENTIAL path, `ctx` is a `SessionContext`, whose `perform_safepoint`
(`src/backend/eval/trampoline/session_context.rs:233`) does:

```
fn perform_safepoint(&self, roots: Vec<MettaValue>) {
    let _root_handle = register_temporary_roots(roots);   // <- registers S/C/K
    request_gc();
    // _root_handle drops here → UNREGISTERS the temporary roots.
}
```

So the slab collector sees the S/C/K because the safepoint registers them as
**temporary roots** (`register_temporary_roots`, `gc_allocator.rs:3777`), which
`collect_all_roots()` reads back via `collect_safepoint_roots()`
(`gc_allocator.rs:3682`/3797). **Critically, the registration is RAII-scoped:
`_root_handle` drops the instant `perform_safepoint` returns, removing the S/C/K
from the registry.** They are only in `collect_all_roots()` *during* the slab
safepoint dance.

### Consequence for the index collector — what we must guarantee

`collect_all_roots()` therefore does **NOT** include the trampoline S/C/K outside
that transient window. If the index collector called `collect_all_roots()` on its
own (after `perform_safepoint` returned, or from a different point), it would
mark from env/tiers/caches only and **miss the live focus expression,
continuation stack, and frame chain → use-after-free.**

**The invariant we MUST satisfy:** at the instant `IndexHeap::mark` runs, the
root set passed to it includes EVERY live `Addr` the trampoline (and
env/tiers/etc.) can still reach.

### What we did (REVISED after empirical UAF discovery — see RESULTS STEP 0)

The mid-loop safepoint turned out to be UNSAFE for a synchronous collector: the
live **bytecode-VM execution stacks** during a nested `eval_trampoline` call
(`vm/mod.rs::eval_sub_expr_vm_all_with_bindings`) are not comprehensively rooted,
and a mid-loop sweep frees them → use-after-free (reproduced and root-caused).
The slab GC tolerates this only because it DEFERS reclaim to true quiescence.

**Final design:** run the collector at TRUE quiescence — the point in `eval()` /
`eval_with_tier` AFTER the `EvalGuard` drops (`active_evaluator_count() == 0`),
where no trampoline loop and no VM is live on the Rust stack, so the complete
root set is `collect_all_roots()` UNIONED with the about-to-be-returned result
values. This is the slab GC's own session-release reclaim point and exactly the
proven `QuiescenceInvariant`. The historical mid-loop-safepoint description below
is retained for context; the implemented root set is:

```
roots_index = root_set.roots()  (the full S/C/K + frame chain + caches + deferred env)
            ∪ collect_all_roots()  (env, tiers, MettaState output, deferred-drop,
                                     parallel-dispatch RootProviders, + any active
                                     safepoint temporary roots)
```

projected to `Vec<Addr>` via `MettaValue::as_arena_addr()` (`filter_map`; inline
scalars and slab values yield `None` and drop out correctly).

This is the union of:
- the EXACT same S/C/K the slab collector marks (we reuse the trampoline's own
  `root_set`, so by construction we cannot under-cover relative to slab), AND
- `collect_all_roots()` for the environment/tier/output roots that live in
  `RootProvider`s rather than on the trampoline stack.

Because `collect_all_roots()` also calls `collect_safepoint_roots()`, any S/C/K
that some *other* (non-existent, in single-threaded mode) evaluator registered
would also be included — harmless redundancy. In single-threaded mode the only
registered safepoint roots, if any, are this thread's, and we include the live
`root_set` directly regardless.

This makes the index root set a **superset** of the slab collector's marked set
on the sequential path, which is exactly the completeness the UAF invariant
requires.

## STEP 1 — Provable-safety gate

A process-global `WORKER_EVER_SPAWNED: AtomicBool` (`gc_allocator.rs`) is set
`true` (Release) at the two eval-worker spawn wrappers (`parallel_dispatch` and
`parallel_collapse_dispatch`, just before `pool.spawn_eval_classified(...)`).

The quiescence collector fires ONLY when (note: `== 0` because the call site is
AFTER the `EvalGuard` drops — true quiescence):

```
gc_mode_is_index() && active_evaluator_count() == 0 && n_threads() == 0
```

Rationale: when both counters are zero, no evaluator or worker native stack can
touch σ. Fanout configuration does not poison this true-quiescence point; it only
blocks the mid-loop non-rendezvous collector, where live control may still exist
on an evaluator stack. At this between-steps safepoint, true quiescence is the
proven `QuiescenceInvariant`, with no admission-gate/rendezvous needed.

This makes the collector safe regardless of `METTATRON_PARALLEL_FANOUT_DEPTH`;
setting `METTATRON_PARALLEL_FANOUT_DEPTH=0` is still useful in validation because
it keeps the mid-loop non-rendezvous collector eligible too.

## STEP 2 — Trigger (committed-bytes watermark)

At the quiescence point, when the gate holds, we decide whether to collect
via an adaptive watermark held in a thread-local-free process global
(`IndexGcWatermark` — single-threaded path, so a plain `AtomicUsize` with relaxed
ops is sufficient and race-free in practice):

```
threshold = max(live_bytes_after_last_cycle * 2, MIN_THRESHOLD)
collect when committed_bytes() > threshold
recompute threshold from live_bytes after each cycle
```

`committed_bytes()` / `live_bytes()` are new accessors:
- `IndexArena::committed_bytes()` = Σ over non-released segments of
  `nodes.capacity() * size_of::<Node>()` (the node-slab footprint actually
  committed). This is monotone between sweeps and drops at segment release.
- `IndexArena::live_node_count()` = Σ over non-released segments of `len`
  (bump high-water), a cheap proxy; `live_bytes ≈ live_node_count *
  size_of::<Node>()`. After a sweep we recompute the threshold from the
  `SweepStats.live` count.
- `IndexHeap` forwards both; `IndexHeapStore::live_bytes()` returns
  `committed_bytes()` (wiring the `Store::live_bytes` that previously
  defaulted to 0).

`MIN_THRESHOLD` defaults to 8 MiB and is overridable via
`METTATRON_INDEX_GC_MIN_BYTES` (used by validation to force the collector to
fire on small workloads).

## STEP 3 — The collection (under the write lock)

When triggered:

1. Build `roots: Vec<MettaValue>` = `root_set.roots()` clone-extended with
   `collect_all_roots()`.
2. Project to `Vec<Addr>` via `as_arena_addr()` (`filter_map`).
3. Take `global_index_heap().write()`.
4. `heap.mark(&addrs)` then `heap.sweep()`.
5. Drop the write lock.
6. `clear_inner_shadow()` on this thread (the only thread) — drops the per-thread
   `MettaValueInner` materialization cache whose `Addr`-keyed entries may now name
   swept (released) segments.
7. Recompute the watermark from `SweepStats.live`.
8. Increment `GC_CYCLES_RUN` (validation observability).

Because allocation ALSO takes `global_index_heap().write()`, holding it across
mark+sweep is mutually exclusive with allocation by construction — no value can
be allocated (and thus no `Addr` minted) while we mark/sweep.

### Why `clear_inner_shadow()` is sufficient and necessary

`INNER_SHADOW` (`metta_value.rs:885`) is a `thread_local!`
`HashMap<u32 /*Addr.raw()*/, Box<MettaValueInner>>` caching the materialized
`MettaValueInner` for a handle, so `inner_ref()` can hand out a `&MettaValueInner`
in index mode. A swept segment's `Addr`s become invalid (the slot may be reused
or the segment released). Any cached `MettaValueInner` keyed by such an `Addr`
must be dropped, else a later `inner_ref()` for a *reused* `Addr` returns a stale
materialization. `sweep()` already clears the heap's ground-SExpr hash-cons; we
add `clear_inner_shadow()` for the per-thread shadow. In single-threaded mode the
collecting thread IS the only thread with a populated shadow, so clearing this
thread's shadow is complete.

## CONSTRAINTS honored

- All new behavior gated on `gc_mode_is_index()` / `#[cfg(feature="index-gc")]`.
  Default (slab) build byte-identical: the collector block is a single
  `if gc_mode_is_index()` guard at the safepoint; in the default build
  `gc_mode_is_index()` is a const-foldable `false` (the `GC_MODE` static inits to
  0 when the feature is off), so the block is dead and the slab hot path is
  unchanged.
- `.expect(...)` over `unwrap()`. No release warnings. No code deleted to
  disable; the parallel path is untouched.
- No stubs/TODOs in what is wired: the single-threaded collector is complete and
  correct. The parallel rendezvous is a separate increment, not a deferral of
  this one.

## Files changed (final)

- `src/backend/eval/cesk/index_arena.rs`: `committed_node_bytes()`,
  `live_node_count()`, `node_size_bytes()` accessors on `IndexArena`.
- `src/backend/eval/cesk/index_heap.rs`: `committed_bytes()` / `live_bytes()`
  forwarders on `IndexHeap`; `IndexHeapStore::live_bytes()` returns
  `committed_bytes()`; the `index_gc` module (`gate_open`,
  `run_collection_if_triggered(&[MettaValue])`, `cycles_run`, `WATERMARK`,
  `GC_CYCLES_RUN`, `MIN_BYTES`/`DISABLE` env knobs).
- `src/backend/models/gc_allocator.rs`: `WORKER_EVER_SPAWNED` flag +
  `worker_ever_spawned()` / `note_worker_spawned()`.
- `src/backend/models/mod.rs`: re-export the two flag accessors.
- `src/backend/eval/trampoline/eval_loop.rs`: `note_worker_spawned()` at the two
  eval-worker spawn wrappers (`parallel_dispatch`, `parallel_collapse_dispatch`).
- `src/backend/eval/mod.rs`: the collector invocation at the post-`EvalGuard`
  quiescence point in `eval()`.
- `src/backend/eval/tier_forced.rs`: the same invocation in `eval_with_tier`
  (covers the forced-tier conformance entry points; the Auto branch defers to
  `eval()`).
- `src/bin/mtt_conformance.rs`: register the cross-directive `all` accumulator as
  temporary roots (GC root contract) + `INDEX_GC_CYCLES_RUN` report hook.
- `src/main.rs`: `INDEX_GC_CYCLES_RUN` report hook.
- `clear_inner_shadow()` (`metta_value.rs`, already `pub(crate)`) wired into the
  collector after sweep.

## Validation

See `docs/cesk-gc/single-threaded-collector-RESULTS.md`.
