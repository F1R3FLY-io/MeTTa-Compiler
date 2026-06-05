# E1-FLIP / CEX-1 — Principled, Fully-Generalized Rendezvous Root Completeness

**Status:** implemented in source; retained as the design record. The described
canonical per-thread contribution, live-dispatch anchors, and rendezvous-union oracle
are present in `roots.rs`, `gc_allocator.rs`, `gc_driver.rs`, `types.rs`, and
`eval_loop.rs`. Later E1 ledgers supersede the original validation/default-flip plan;
the dedicated collector remains opt-in and the default flip is still gated.

**Implementation audit (2026-06-04):** the stale WIP patch
`docs/cesk-gc/wip-cex1-principled-impl.patch` no longer applies because its edit sites
are already represented in current source: `ThreadContribution` /
`collect_complete_thread_contribution`, `collect_live_dispatch_anchors`,
`register_live_dispatch`, `snapshot_live_dispatch_witness`,
`collect_live_dispatch_anchors(&mut roots)`, and `assert_rendezvous_union_complete`.
Do not reapply the patch.

## Why (the anti-fragility mandate)

The GC migration exists to ELIMINATE "remember to register/enumerate roots." The
dedicated-GC-thread ("rendezvous") concurrent collector regressed into that fragility
in two non-principled ways, both of which this design removes:

1. **Per-site source enumeration** — each park/finish/safepoint site manually listed
   `collect_eval_memo_roots`+`collect_match_result_roots`+`collect_subgoal_roots`+
   `collect_thunk_roots`+`collect_binding_capture_roots`+`collect_k_spine`+
   `collect_global_anchors`. Adding a thread-local source ⇒ remember it at EVERY site.
2. **Park-timing-dependent coverage** — a worker blocked at `EvalGuard::enter`'s
   admission gate (decremented `ACTIVE_EVALUATORS`, parked on `GC_PROGRESS_CONDVAR`,
   gc_allocator.rs ~3712) is NOT a rendezvous participant and never self-roots, yet
   holds a live closure capture (`branch_expr`/`branch_bindings`) reachable only
   through the parent's `Continuation::WaitForParallel` fan-out — covered only IF the
   parent happens to park that frame this cycle. The V4 gate proved this corrupts
   (robot PLN ~11/16 wrong; WIP per-site fix → 7/16; this design → 0/16).

## Root cause (proven, Addr-correlation)

The dedicated GC thread marks ONLY from `drain(WORKER_ROOT_BUFFER) ∪
collect_safepoint_roots()`; unlike the slab collector it does NOT run
`collect_k_spine`/the caches during its own mark. So mark-completeness depends on what
each mutator PUBLISHES + what the driver walks. Two holes (proven: hole (2) refuted —
`queue_len(pending)=0` at the mark; hole (1) confirmed): live `Addr`s held in
thread-local/native holders not published, and the dispatch fan-out not GC-thread-walked.

## The principled split (load-bearing insight)

| Root source | Lives in | Who can read | ⇒ |
|---|---|---|---|
| C/E_local/K registers, K-spine, the 4 thread-local caches (EVAL_MEMO, MATCH_RESULT, subgoal, thunk), binding-capture | `thread_local!` / native stack | ONLY the owning thread | self-publish via ONE canonical collector |
| dispatch fan-out (`branches: Arc<Vec<ParallelBranch>>`, `results: Arc<Mutex<…>>`), E₀ | shared `Arc` (Send+Sync) | the GC thread itself | GC-thread-walked structural root, park-timing-independent |

`MettaValue` is a `Copy` 8-byte `Addr`; the caches are behind `thread_local!`, so the
GC thread physically cannot reach thread B's `EVAL_MEMO` → B must self-publish. The
fan-out is already `Arc`-shared → the GC thread can walk it (the slab build did, via
`ParallelDispatchRootProvider`; A5 deleted the registry and never replaced the walk —
that deletion is the residual bug).

## D1 — the canonical per-thread collector (`roots.rs`, beside `collect_machine_roots_live` :373)

```rust
#[cfg(feature = "index-gc")]
pub enum ThreadContribution<'a> {
    Trampoline { operand_stack, current_work, work_stack, continuations, env0, deferred_envs, extra }, // sites #1,#3,#4,#5
    TierLeaf { extra },                                                                                 // site #2 (outside the loop)
}
#[cfg(feature = "index-gc")]
pub fn collect_complete_thread_contribution(out: &mut Vec<MettaValue>, ctx: ThreadContribution<'_>);
```
- `Trampoline` body = `out.extend(extra); collect_machine_roots_live(out, …); for e in deferred_envs { e.collect_roots_into(out); }`.
- `TierLeaf` body = `out.extend(extra); collect_persistent_roots_via_global_env0(out);` (E₀ ∪ anchors ∪ K-spine; no in-scope S/C/K).
- **KEY:** `collect_machine_roots_live` ALREADY transitively contains the WIP's 7 collectors (memo/match/subgoal/thunk are in `collect_global_anchors` :301; +k-spine in `collect_persistent_roots` :404; both in `collect_machine_roots_live` :386). So the WIP per-site lists were redundant — the fix is "all sites call the COMPLETE canonical collector," not "add sources per site." Fold the lone `binding_capture` into `collect_global_anchors` once.
- **Anti-fragility:** new thread-local source ⇒ one line in `collect_global_anchors` (the ST collector already requires it there); all sites inherit it. Two enum variants capture the only two register-provenance shapes (compile-checked; a missing field is a compile error).

## D2 — the global dispatch-fan-out anchor (`gc_allocator.rs`, modeled on `SAFEPOINT_ROOTS` :4636)

```rust
#[cfg(feature="index-gc")] pub trait DispatchRoots: Send + Sync { fn collect_dispatch_roots(&self, out: &mut Vec<MettaValue>); }
#[cfg(feature="index-gc")] static LIVE_DISPATCHES: OnceLock<Mutex<Vec<Option<Weak<dyn DispatchRoots>>>>>;
#[cfg(feature="index-gc")] pub struct LiveDispatchHandle { idx: usize }   // Drop frees the slot (RAII, like SafepointRootHandle)
#[cfg(feature="index-gc")] pub fn register_live_dispatch(d: &Arc<dyn DispatchRoots>) -> LiveDispatchHandle;  // store Arc::downgrade
#[cfg(feature="index-gc")] pub fn collect_live_dispatch_anchors(out: &mut Vec<MettaValue>);                 // upgrade+walk each; prune dead
```
- `Weak` (never extend lifetime; self-healing on panic-without-deregister, like the slab ROOT_REGISTRY).
- Reuse the EXISTING `ParallelDispatchRootProvider` (types.rs:287) / `ParallelCollapseRootProvider` (:388): `impl DispatchRoots` body = their slab `collect_roots` (inputs from the immutable `branches`/`items` Arc + outputs via `results.try_lock()`). They are already constructed in both builds + held via `_root_provider_arc`.
- Register at `parallel_dispatch` (~2736) / `parallel_collapse_dispatch` (~3274) in an `#[cfg(index-gc)]` arm; store the `LiveDispatchHandle` in a new `#[cfg(index-gc)] _live_dispatch` field on the handle → RAII deregister when `WaitForParallel` is consumed.
- Driver drains it: `gc_driver.rs:186`, add `ga::collect_live_dispatch_anchors(&mut roots);` after `collect_safepoint_roots`.
- **Covers class-2 timing-independently:** the worker closure captures `branch_expr` by move, but the SAME `Addr` is co-held by `root_provider.branches` (`Arc::clone`, one alloc / two strong refs) → walked by the anchor whether the worker is admission-blocked, not-yet-started, or running; independent of which thread holds `WaitForParallel`.
- **Soundness (NOT a discovery side-channel):** `LIVE_DISPATCHES` is the reification of the live parallel-K tree's fork nodes (`WaitForParallel`). Sequential K = one native stack (walked by `collect_k_spine`); forked K = a tree; the pending `(expr,bindings)` are un-entered sub-continuation inputs, `results[slot]` the completed outputs — both `σ|_Reachable` of the parallel K. The set of live dispatches IS machine state (in-flight parallel continuations), read structurally by name, bounded shape — the parallel analogue of `collect_k_spine`'s `SUSPENDED_ACTIVATIONS`. Nothing opts in except the dispatch op; what it yields is determined by K-structure. Passes the same bar `collect_k_spine` passed.

## Completeness theorem (concurrent), discharged

`R = drain(WORKER_ROOT_BUFFER) ∪ collect_safepoint_roots() ∪ collect_live_dispatch_anchors()`.
Partition every live Addr-holder at `gate_open_rendezvous()`==true: (1) participating
threads (in `n`) — published complete via D1 under HB2; (2) non-participating
dispatch-capture holders (admission-blocked / not-yet-started) — walked via D2; (3)
driver program C — `SAFEPOINT_ROOTS` (kept); (4) E₀/anchors — published N× (sound) +
in D2's reach. No fifth class. ∎ `reachable(R) ⊇ every live value` ⇒ sweep reclaims
only dead.

## D5 — the permanent machine-equivalence ORACLE (the standing guarantee)

`#[cfg(debug_assertions)]` `assert_rendezvous_union_complete`, in
`gc_driver_rendezvous_cycle` after drain (gc_driver.rs:186), before the collect:
assert (a) `collect_live_dispatch_anchors() ⊇ ⋃ live-handle (branches ∪ results)` (every
registered dispatch was walked); (b) `|drained participants| == n_threads_at_snapshot`
(every counted thread published — a shortfall is a NAMED panic, not a silent under-mark).
The thread-local half is discharged by D1 being exercised at the slab site #5, where the
EXISTING A4.3 oracle (eval_loop.rs:3879) runs the byte-for-byte check. Permanent CI
invariant (like A4.3); zero-cost in release. ⇒ a future forgotten source / unregistered
dispatch trips a debug panic, not a corruption bug.

## Edit list

- **roots.rs:** `ThreadContribution` + `collect_complete_thread_contribution` (~:373); fold `binding_capture` into `collect_global_anchors` (:316); update `assert_quiescence_superset` (:432) NEW term to the canonical reader.
- **gc_allocator.rs:** `LIVE_DISPATCHES` + `DispatchRoots` + `LiveDispatchHandle`/Drop + `register_live_dispatch` + `collect_live_dispatch_anchors` (~:4636, mirror SAFEPOINT_ROOTS).
- **gc_driver.rs:** +`collect_live_dispatch_anchors(&mut roots)` at :186; +`assert_rendezvous_union_complete` (debug).
- **types.rs:** `impl DispatchRoots` for both providers (~:317/:397, body = slab collect_roots); +`#[cfg(index-gc)] _live_dispatch: Option<LiveDispatchHandle>` on both handles (~:228/:363).
- **eval_loop.rs:** REVERT the WIP per-site blocks; the 5 sites become ONE canonical call each (#1 ~4052, #2 ~204 TierLeaf, #3 ~2631 finisher Trampoline+extra=result, #4 ~3209, #5 ~3843 slab/midloop — unifies slab+dedicated); the 2 registrations (~2736/~3274) + the new handle field in the struct literals (~2761/~3291); update A4.3 oracle NEW term (~3879) to the canonical reader.

## Validation ladder (each gate green before the next)

1. Build both backends; 49-warning baseline unchanged (cfg-walls keep slab providers from `dead_code`).
2. FANOUT=0 conformance 483/0 byte-identical on both backends + DEDICATED∈{0,1} (anchor empty at FANOUT=0; `register_live_dispatch` never reached — dispatch needs ≥2 branches).
3. **3-way discriminator, robot @ FANOUT=8, ×16/arm, ALL → 0/16**, reuse ON + `reclaimed_slots>0`: A (DEDICATED=1 MIN_BYTES=131072), B (DEDICATED=0), C (DEDICATED=1 MIN_BYTES=4294967295). Re-run on 2026-06-04 after the literal-closure-entry `CompletionGuard` fix: PASS, all three arms `0/16 FAIL`, arm A reclaimed and released segments.
4. V4 ASAN (`scripts/e1_flip_v4_asan.sh`, robot+raven+stress_multidir @ FANOUT=8 DEDICATED=1): 0-UAF + rendezvous-cycles>0 non-vacuous + non-rendezvous==0. Heed the script's caps (24G/20G, MemorySwapMax=0, -j4; `free -h` first).
5. ×20 determinism (robot @ FANOUT=8 DEDICATED=1, byte-identical canonical output).
6. The new oracle (debug) over the discriminator + V4 fixtures: 0 oracle panics.

CEX-1 itself is committed. The 1-line default flip remains reserved for explicit user
approval after the current E1/R-FL gates are green. Rollback for the opt-in path remains
zero-code: `METTATRON_INDEX_GC_DEDICATED=0`.

## Risks
- `collect_live_dispatch_anchors` lock order: `LIVE_DISPATCHES.lock()` then per-handle `results.try_lock()` (never `lock` — a contended `results` ⇒ a worker mid-write holding its EvalGuard = a class-1 participant who self-rooted that value; skip is safe). Driver holds no other lock at :186.
- `Weak` upgrade race at deregistration: upgrade-succeeds (over-count, sound) or fails (complete, results in parent K) — both sound.
- Byte-identical dormant: all `#[cfg(index-gc)]` + `dedicated_gc_enabled()`; FANOUT=0 never reaches a dispatch registration site, and `n_threads()>1` is intentionally not used there because workers may not have entered yet.
- Over-count (E₀ N×, all live dispatches): sound (dedup at the `as_arena_addr` mark projection); throughput-only, measured at Phase F.
- The original expectation was that this would close the `stress_multidir` recycle panic;
  later validation reclassified that fixture as a pre-existing FANOUT=8 baseline crash
  tracked separately in `e1-flip-collapse-worker-env-gap-2026-06-03.md`.
