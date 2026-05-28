# Single-Threaded Store-Centric Collector — Validation Results

Date: 2026-05-28. Design: `docs/cesk-gc/single-threaded-collector.md`.
Hardware-cell: `systemd-run --user --scope -p MemoryMax=… -p CPUQuota=400%`.

## Executive summary

The FIRST WORKING store-centric collector is implemented and validated. It runs
the already-correct `IndexHeap::mark`/`sweep` core as a live collector at the
**true-quiescence reclaim point** (post-`EvalGuard`, `active_evaluator_count() ==
0`), gated to the provably-single-threaded regime (`!worker_ever_spawned()`),
behind `--features index-gc`. The default (slab) build is unaffected.

| Validation | Result |
|---|---|
| 1. Default (slab) build unaffected | `cargo nextest run --release`: **4312 passed, 0 failed**; conformance **483/221/40**, `INDEX_GC_CYCLES_RUN=0` (collector inert in slab) |
| 2. Correctness under collector ON | `--features index-gc`, `FANOUT_DEPTH=0`, `MIN_BYTES=262144`: conformance **483 / M11-pt 221 / M11-he 40**, FAIL=0, **`INDEX_GC_CYCLES_RUN=840`** |
| 3. Reclamation (RSS bounded) | 8000-directive workload, tier 0: collector **ON peak RSS 1.40 GB** (170 cycles) vs **OFF peak RSS 9.98 GB** — **7.14× / 8.38 GB reclaimed**, identical output |
| 4. ASAN UAF gate | nightly `-Zsanitizer=address` `-Zbuild-std`, `--features index-gc`: full conformance **rc=0, 483/221/40, 840 cycles, ZERO ASAN errors**; 120-directive stress **rc=0, 53 cycles, clean** |
| index-gc nextest (cesk/gc/index subset) | **706 passed, 0 failed** |

## STEP 0 finding (the UAF linchpin)

The slab collector obtains the sequential trampoline's S/C/K at the periodic
safepoint (`eval_loop.rs:3386`) by building a `RootSet` (work items +
continuations + frame chain + pointer-keyed caches + deferred-env) and passing it
to `SessionContext::perform_safepoint` (`session_context.rs:233`), which
**RAII-registers it as temporary roots** for the duration of the safepoint dance
and then drops it. `collect_all_roots()` reads those back via
`collect_safepoint_roots()` — but ONLY during that transient window.

**Empirical discovery (the decisive part):** a synchronous mark-sweep at the
mid-loop safepoint is NOT safe, because the live **bytecode-VM execution stacks**
(`value_stack` / `locals` / `results` / `current_bindings`) that exist when the
VM calls a nested `eval_trampoline` (`vm/mod.rs:8569`,
`eval_sub_expr_vm_all_with_bindings`) are NOT comprehensively registered as
roots. The slab GC tolerates this only because it DEFERS reclaim to true
quiescence (`ACTIVE_EVALUATORS == 0`). A mid-loop sweep frees those VM-stack
values → use-after-free. This was reproduced (`get` on a released segment, raw
20184912 / seg 76) and root-caused via a two-pass watch+backtrace
(`alloc`-site capture pointed at `AddOp::execute_step` inside the VM, called
through a nested `eval_trampoline` whose inner safepoint swept the VM's stacks).

**Resolution:** collect at TRUE quiescence — the point in `eval()` /
`eval_with_tier` AFTER the `EvalGuard` drops, where no trampoline loop and no VM
is live on the Rust stack, so the complete root set is `collect_all_roots()`
(env / tiers / promoted RootProviders) UNIONED with the about-to-be-returned
result values. This is exactly the slab GC's session-release reclaim point and
exactly the proven `QuiescenceInvariant`.

**Second incompleteness, also empirically found + fixed:** callers accumulate
result `MettaValue`s across directives in Rust-local Vecs that are not in any
`RootProvider` (e.g. the conformance harness's `all: Vec<MettaValue>`). A
per-directive collection freed those → 3 conformance fixtures regressed
(`016-min-max-atom`, `008-rule-via-add-atom`, `421-catch-3arg-pt`). Fix: the
harness registers its accumulator via `register_temporary_roots(all.clone())`
each iteration (the legitimate GC root contract). The CLI is unaffected because
it formats results to strings immediately (no cross-directive MettaValue
accumulation). After rooting the accumulator: 483/221/40, FAIL=0.

## Validation 1 — default (slab) build unaffected

```
cargo build --release                  → clean, no new warnings from changed files
cargo nextest run --release            → Summary [7.020s] 4312 tests run: 4312 passed, 0 skipped
mtt-conformance --strict (slab binary) → Summary: 483 pass, 0 fail, 0 error, 0 skipped
                                          mtt=483 M11-pt=221 M11-he=40 FAIL=0
                                          INDEX_GC_CYCLES_RUN=0  (collector inert in slab build)
```

The slab path is byte-identical: `gate_open()`'s first conjunct
`gc_mode_is_index()` is a relaxed load of a write-once static that inits to 0
when the feature is off, so the collector call is a single perfectly-predicted
false branch at the (cold) post-EvalGuard site — off the reduction hot path
entirely. `cycles_run() == 0` confirms it never fired.

## Validation 2 — correctness under the collector ON

```
cargo build --release --features index-gc
METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=262144 \
  METTATRON_INDEX_GC_REPORT=1 mtt-conformance --strict
→ EXIT=0
  Summary: 483 pass, 0 fail, 0 error, 0 skipped
  mtt=483  M11-pt=221  M11-he=40  FAIL=0
  INDEX_GC_CYCLES_RUN=840
```

With `FANOUT_DEPTH=0` the single-threaded gate stays open, so the collector
actually fired **840 times** during the run and conformance is byte-identical to
the slab baseline. A wrongly-swept live value would crash or mis-evaluate; none
did.

## Validation 3 — reclamation proof (RSS bounded)

Workload `examples/cesk-gc/stress_multidir.metta`: 8000 `!(burn 200)` directives,
each building+folding a depth-200 Peano wrapper (all transient, dead at the
directive's quiescence) and returning `0`. Tier 0.

```
ON  : METTATRON_INDEX_GC_MIN_BYTES=2097152  → peak RSS 1,397,508 KB (1.40 GB), 170 cycles, 8000/8000 = [0]
OFF : METTATRON_INDEX_GC_DISABLE=1          → peak RSS 9,975,700 KB (9.98 GB),            8000/8000 = [0]
OFF/ON ratio = 7.14×  (collector reclaimed 8.38 GB)
```

With the collector disabled the index arena grows monotonically to ~10 GB; with
it enabled, peak RSS stays bounded at ~1.4 GB — same correct output in both arms.
This proves the collector reclaims.

(Note: a small residual growth remains under ON because the eval-memo / tiered
cache RootProviders legitimately pin a growing set of cached values — those are
reachable roots the collector correctly retains; that is a caching-policy
concern, not a GC bug. The 7.14× reduction is the collector reclaiming the true
transient garbage.)

## Validation 4 — ASAN use-after-free gate (decisive safety check)

```
RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
  cargo +nightly build --features index-gc -Zbuild-std --target x86_64-unknown-linux-gnu
```

Full conformance under ASAN, collector firing:
```
METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 \
  ASAN_OPTIONS=detect_leaks=0:abort_on_error=1  mtt-conformance --strict
→ rc=0
  Summary: 483 pass, 0 fail, 0 error, 0 skipped   (mtt=483 M11-pt=221 M11-he=40 FAIL=0)
  INDEX_GC_CYCLES_RUN=840
  ASAN errors: NONE  (no use-after-free / heap-use-after-free / poison / released-segment access)
```

120-directive stress under ASAN: `rc=0, 120/120 = [0], 53 cycles, ZERO ASAN
errors`. The dev profile keeps `debug_assert!(!seg.released)` in `IndexArena::get`
active, so any dangling access would abort; none occurred. This confirms
root-set completeness at the quiescence point.

## index-gc in-tree tests

```
cargo nextest run --release --features index-gc                  → 4166 tests run: 4166 passed, 0 skipped
cargo nextest run --release  (default/slab)                      → 4314 tests run: 4314 passed, 0 skipped
```

(+2 over the historical 4312 = the two new collector unit tests in `index_heap.rs`:
`committed_bytes_drop_after_segment_release` asserts the watermark signal drops
on segment release; `gate_closes_after_worker_spawned` asserts the safety gate
latches shut once any worker is noted.)

### Perf note (per-eval overhead)

A first cut called `collect_all_roots()` on EVERY `eval()` quiescence; under the
(slower, in-progress) index store path this pushed several heavy regression test
FILES past nextest's 120 s timeout (29 TMT, ZERO assertion failures). Fix: a
cheap `index_gc::should_collect()` pre-check (gate + committed-bytes watermark,
no provider walk) gates the call sites, so the expensive `collect_all_roots()`
root build happens ONLY when a collection will actually fire. After the fix the
full index-gc suite completes in ~7.5 s, 4166/4166.

## Diff summary (the wiring)

- `src/backend/eval/cesk/index_arena.rs`: `committed_node_bytes()`,
  `live_node_count()`, `node_size_bytes()` accessors.
- `src/backend/eval/cesk/index_heap.rs`: `committed_bytes()` / `live_bytes()`
  forwarders; `IndexHeapStore::live_bytes()` → `committed_bytes()`; the
  `index_gc` module (`gate_open`, `run_collection_if_triggered`, `cycles_run`,
  `WATERMARK`, `GC_CYCLES_RUN`, `MIN_BYTES`/`DISABLE` env knobs).
- `src/backend/models/gc_allocator.rs`: `WORKER_EVER_SPAWNED` flag +
  `note_worker_spawned()` / `worker_ever_spawned()`.
- `src/backend/models/mod.rs`: re-export `note_worker_spawned` / `worker_ever_spawned`.
- `src/backend/eval/trampoline/eval_loop.rs`: `note_worker_spawned()` at the two
  eval-worker spawn wrappers (`parallel_dispatch`, `parallel_collapse_dispatch`).
- `src/backend/eval/mod.rs`: quiescence-point collector hook in `eval()`.
- `src/backend/eval/tier_forced.rs`: quiescence-point collector hook in
  `eval_with_tier` (covers the forced-tier conformance entry points).
- `src/bin/mtt_conformance.rs`: register the cross-directive `all` accumulator as
  temporary roots (GC root contract) + `INDEX_GC_CYCLES_RUN` report hook.
- `src/main.rs`: `INDEX_GC_CYCLES_RUN` report hook.
- `examples/cesk-gc/{stress_alloc,stress_multidir}.metta`: validation workloads.

## Honest scope / residual concerns

- **Collection frequency:** the collector fires at each top-level `eval()` /
  directive quiescence, not mid-trampoline. A single giant `!(...)` directive
  therefore collects only once (at its end); RSS during one such expression is
  not bounded incrementally. Multi-directive programs (the mmverify / conformance
  shape) reclaim between directives — validated.
- **Comprehensive mid-execution VM rooting** (which would enable mid-loop
  collection) is the documented broader increment and is deliberately OUT of
  scope here.
- **Parallel rendezvous** is out of scope: the moment any eval worker is spawned,
  `worker_ever_spawned()` latches and the collector backs off entirely.
- **Caller root contract:** any NEW caller that accumulates `MettaValue`s across
  `eval` calls in a Rust-local must register them as temporary roots (as the
  conformance harness now does), or run with the collector disabled. This is the
  standard GC-root obligation, documented here so it is not a silent trap.
