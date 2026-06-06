# Mid-Execution Rooting & Mid-Loop Collection (Inc 6b-1)

Status (updated 2026-06-06): **structural mid-loop root-union proof committed
(`b81eeff`), still OPT-IN / default-OFF pending forced-ASAN validation.** Enable
with `METTATRON_INDEX_GC_MIDLOOP=1`. Until the ASAN root-completeness gate is
green, shipped `--features index-gc` behavior remains the already-validated Inc
6a true-quiescence collector (see `single-threaded-collector.md`).

## 1. Motivation — the mid-loop use-after-free

The Inc 6a collector reclaims only at **true quiescence** — the post-`EvalGuard`
point in `eval()` / `eval_with_tier` where `active_evaluator_count() == 0` and no
trampoline loop or bytecode-VM is live on the Rust stack. There the complete root
set is simply `collect_all_roots() ∪ {about-to-be-returned results}`.

That leaves one limitation: **a single large `!(…)` directive collects only once,
at its end.** RSS is not bounded *during* one big expression. Lifting that needs
collection at a **mid-loop safepoint** — *while* the trampoline (and possibly a
nested bytecode VM) is live.

The agent that first attempted mid-loop collection found (empirically, via a
reproduced ASAN use-after-free — `get` on a released segment, root-caused to
`AddOp::execute_step` inside a VM reached through a nested `eval_trampoline`) that
**the live bytecode-VM execution stacks are not registered as GC roots**. When the
VM evaluates a sub-expression it calls a nested `eval_trampoline`
(`eval_sub_expr_vm_all_with_bindings`, `vm/mod.rs`); the *outer* VM's
`value_stack` / `locals` / `results` / `current_bindings` / `choice_points` hold
live `MettaValue`s that no `RootProvider` and no trampoline `RootSet` enumerated.
A sweep fired inside that nested trampoline frees them → UAF.

The 6a collector is immune only because it defers to true quiescence, where no VM
is on the stack. Mid-loop collection has no such luxury — it MUST root every live
on-stack VM frame.

## 2. Mechanism — structural K-spine roots

The current index build does not use the legacy `frame_chain` discovery channel.
At a mid-loop safepoint it builds roots structurally from the reified machine:

```
  collect_machine_roots_live
      = live S/C/K
        ∪ reach(E₀)
        ∪ collect_global_anchors()
        ∪ collect_k_spine()

  midloop_roots
      = collect_machine_roots_live
        ∪ deferred environment drops
        ∪ SAFEPOINT_ROOTS driver-C channel
```

`collect_k_spine()` walks typed suspended trampoline activations and VM leaves.
`GenericBytecodeVM::with_vm_roots_frame()` is split by build: the slab build keeps
the legacy frame-chain frame, while the index build pushes a typed K-spine
`VmLeaf` decoded by `collect_k_spine()` through the same `collect_roots_into`
reader. This is what makes nested VM stacks visible without reintroducing a root
registry or a raw discovery side-channel.

## 3. The mid-loop safepoint

`eval_loop.rs` mid-loop branch:

```rust
if index_gc::should_collect_midloop() {
    let mut midloop_roots = Vec::with_capacity(root_set.len() + 64);
    collect_machine_roots_live(&mut midloop_roots, ...); // live S/C/K + E0/global/K-spine
    for deferred_env in &deferred_shared_drops {
        deferred_env.as_ref().collect_roots_into(&mut midloop_roots);
    }
    collect_safepoint_roots(&mut midloop_roots); // driver-C
    index_gc::run_collection_if_triggered_midloop(&midloop_roots);
}
```

The formal `MidloopRootUnion` proof and TLA discriminator pin this union. The
source-coupling harness also checks that the implementation builds the vector in
that order and passes the same vector to `run_collection_if_triggered_midloop`.
ASAN must still validate the operational default-on gate by forcing real
mid-execution sweeps (a missed root → a value freed mid-execution →
heap-use-after-free).

The collection body (`mark_sweep_if_over_watermark`) is IDENTICAL to the
quiescence path (shared core); only the gate and the root set differ.

## 4. Safety gate

`index_heap::index_gc::gate_open_midloop()` (`index_heap.rs:860`):

```
gc_mode_is_index() && midloop_enabled() && !worker_ever_spawned()
                   && active_evaluator_count() == 1 && !disabled()
```

- `gc_mode_is_index()` — const-false in the slab build ⇒ the whole mid-loop block
  is a single perfectly-predicted dead branch off the hot path.
- **`midloop_enabled()`** — `METTATRON_INDEX_GC_MIDLOOP=1`, **default OFF** (the
  opt-in introduced in `a05adc2` after the OOM incident; see §6).
- `!worker_ever_spawned() && active_evaluator_count() == 1` — the
  provably-single-threaded regime. At a mid-loop safepoint *inside* the trampoline
  the sole evaluator holds exactly one `EvalGuard` (count is **1**, not 0). With no
  worker ever spawned, no parked-resumable worker can exist, so the single calling
  thread is the SOLE thread that can touch σ — the trivially-true instance of the
  TLA+-proven `QuiescenceInvariant`. The mark+sweep runs under the heap write lock,
  mutually exclusive with allocation, so no `Addr` is minted mid-mark.

(Contrast 6a's quiescence gate, which requires `active_evaluator_count() == 0`.)

## 5. Trigger

`should_collect_midloop()` is a cheap pre-check (gate + committed-bytes watermark)
so the expensive complete-root-set build happens only when a collection will fire.
On the conformance suite the mid-directive committed bytes never exceed the
watermark (small directives + the 6a quiescence collector keeps σ small between
directives), so **mid-loop fires 0 times on conformance** — it is for large single
directives.

## 6. Why default-OFF — the OOM incident (2026-05-28)

The agent implementing this increment ran its validation — a nightly ASAN
`-Zbuild-std` build plus stress workloads — **in the background, uncapped**, and
OOM-crashed the 125 GiB machine before the ASAN proof completed. No results doc
was produced; mid-loop is therefore **unvalidated**.

Decision (done-right): rather than ship default-on unvalidated potentially-UAF
behavior, mid-loop was made **opt-in** (`midloop_enabled()`, default OFF) — a
small, non-destructive gate that preserves all of the agent's code. The default
`index-gc` build then behaves exactly as the ASAN-validated 6a collector.

Standing lesson (memory: `resource-limits-heavy-ops`): every heavy op
(build / ASAN / nextest / stress / massif) runs under `systemd-run -p MemoryMax=…`;
never background an uncapped build. ASAN `-Zbuild-std` is the single most
memory-hungry op here.

## 7. Validation plan (to complete Inc 6b-1)

1. Build nightly ASAN **capped**:
   `systemd-run --user --scope -p MemoryMax=48G -p CPUQuota=… RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" cargo +nightly build --features index-gc -Zbuild-std --target x86_64-unknown-linux-gnu`
   (serial / low `-j`; size the cap against the *full* machine when no other build
   is running).
2. Force mid-loop to fire: `METTATRON_INDEX_GC_MIDLOOP=1
   METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=<small>` on
   (a) the full conformance suite and (b) a **single giant `!(…)` directive** that
   builds + discards heavy transient garbage mid-evaluation (so mid-loop sweeps
   fire while VM frames are live). `INDEX_GC_MIDLOOP_CYCLES` must be > 0.
3. **Pass = 0 ASAN errors** (root completeness dynamically validated) AND conformance still
   483 / 221 / 40. Then flip `midloop_enabled()` default to on (or drop the env
   gate) + re-confirm + commit.
4. If an ASAN UAF appears: a live value class is unrooted — extend
   `collect_roots_into` / K-spine coverage; do not flip on.

## 8. Relation to the parallel collector (Inc 6b-2)

This same typed K-spine VM rooting is a prerequisite for the parallel collector:
each parked worker has its own nested trampoline/VM stack, and the rendezvous
collector relies on each worker self-publishing the structural roots for its own
machine.
