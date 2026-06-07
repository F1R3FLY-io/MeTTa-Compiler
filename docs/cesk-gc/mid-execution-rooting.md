# Mid-Execution Rooting & Mid-Loop Collection (Inc 6b-1)

Status (updated 2026-06-07): **structural mid-loop root-union proof committed
and the focused forced-ASAN root-completeness gate is green.** The path is
default-on inside the proven single-evaluator gate; there is no mid-loop feature
switch.

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
The focused ASAN gate now forces a real mid-execution minor while this structural
root vector is live; a missed root would turn into a value freed mid-execution
and then a heap-use-after-free.

The collection body (`mark_sweep_if_over_watermark`) is IDENTICAL to the
quiescence path (shared core); only the gate and the root set differ.

## 4. Safety gate

`index_heap::index_gc::gate_open_midloop()` (`index_heap.rs:860`):

```
gc_mode_is_index() && !worker_ever_spawned()
                   && active_evaluator_count() == 1 && !disabled()
```

- `gc_mode_is_index()` — const-false in the slab build ⇒ the whole mid-loop block
  is a single perfectly-predicted dead branch off the hot path.
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

## 6. Why There Is No Mid-Loop Switch

The agent implementing this increment originally ran its validation — a nightly ASAN
`-Zbuild-std` build plus stress workloads — **in the background, uncapped**, and
OOM-crashed the 125 GiB machine before the ASAN proof completed. That historical
failure justified holding the path back until the proof and ASAN gate were real.

Now that the structural proof, TLC discriminator, source-coupling check, and
focused forced-ASAN run are green, keeping a mid-loop configuration knob would
only preserve a debugging artifact. The design is the structural CESK collector:
when the single-evaluator safety gate holds and the watermark is due, the
mid-loop collector runs.

Standing lesson (memory: `resource-limits-heavy-ops`): every heavy op
(build / ASAN / nextest / stress / massif) runs under `systemd-run -p MemoryMax=…`;
never background an uncapped build. ASAN `-Zbuild-std` is the single most
memory-hungry op here.

## 7. Validation result

Focused root-completeness gate, capped and foregrounded:

```
scripts/d_midloop_asan.sh
```

Result on 2026-06-07:

- ASAN build: rc 0.
- `default` arm (FANOUT depth 0): rc 0, 0 ASAN/UAF lines, 1 minor cycle, 0 major cycles, 0 read-site assertion panics, result `[done]`.
- Verdict: arms with UAF 0; midloop minors 1, so the gate was non-vacuous.
- Release strict conformance after the default-on flip: 483 pass, 0 fail, 0 error, 0 skipped, with
  `INDEX_GC_CYCLES_RUN=798` (`METTATRON_INDEX_GC_MAX_BYTES=1048576`) and `INDEX_GC_MIDLOOP_CYCLES=0`. This validates
  the default executable path and the broader semantic oracle; the focused ASAN fixture above is the non-vacuous
  mid-loop sweep witness.

The script now hard-fails if the ASAN build fails, if an arm fails, if UAF/panic/error lines appear, if the result is
not `[done]`, or if the default mid-loop arm does not run at least one minor cycle. This result discharges the focused
mid-execution root-completeness ASAN check.

## 8. Relation to the parallel collector (Inc 6b-2)

This same typed K-spine VM rooting is a prerequisite for the parallel collector:
each parked worker has its own nested trampoline/VM stack, and the rendezvous
collector relies on each worker self-publishing the structural roots for its own
machine.
