# Mid-Execution Rooting & Mid-Loop Collection (Inc 6b-1)

Status (2026-05-28): **infrastructure committed (`a05adc2`), OPT-IN / default-OFF,
ASAN validation PENDING.** Enable with `METTATRON_INDEX_GC_MIDLOOP=1`. Until the
ASAN root-completeness proof lands, the shipped `--features index-gc` behavior is
the already-validated Inc 6a true-quiescence collector
(see `single-threaded-collector.md`).

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

## 2. Mechanism — VM frames as frame-chain root providers

The fix threads each on-stack VM frame's roots into the existing thread-local
**evaluation frame chain** (`src/backend/eval/frame_chain.rs`), which the
trampoline's `RootSet` already walks at safepoints.

```
  eval_trampoline (mid-loop safepoint) ── walks ──▶ FRAME_CHAIN_HEAD
        ▲                                                 │
        │ nested call                                     ├─▶ EvalFrame{Eval}
   VM::eval_sub_expr_vm_all_with_bindings                 ├─▶ EvalFrame{BytecodeVm} ─┐
        │                                                 │        root_collector ───┤
        └── pushes guard: with_vm_roots_frame() ──────────┘                          │
                                                                                     ▼
                                                      vm_roots_collector(&VM, out)
                                                        → VM::collect_roots_into(out)
                                                        (value_stack ∪ locals ∪
                                                         results ∪ current_bindings ∪
                                                         choice_points)
```

Pieces (all in `a05adc2`):

- **`FrameLabel::BytecodeVm`** (`frame_chain.rs:104`) — a frame-chain label, with a
  `Display` arm (`"bytecode-vm"`).
- **`GenericBytecodeVM::with_vm_roots_frame(&self)`** (`vm/mod.rs:1181`) — pushes an
  `EvalFrameGuard::push_custom(BytecodeVm, self as *const (), vm_roots_collector)`.
  `TypeId`-gated to `V == MettaValue` (the only monomorphization that reaches a
  nested trampoline; the JIT re-entry is gated OFF under `index-gc`). Returns an
  RAII guard that pops the frame when the nested call returns.
- **`vm_roots_collector(data, out)`** (`vm/mod.rs:144`) — `unsafe fn` that casts
  `data` back to `&GenericBytecodeVM<MettaValue, ActiveFactory>` and calls
  `collect_roots_into(out)`. **Safety:** `self` outlives the guard (sibling stack
  local); the collector reads `&self` only and runs synchronously on the same
  thread inside the nested trampoline, so it never aliases a live `&mut self`.
- **Call site** (`vm/mod.rs:8564`): `let _vm_roots_guard = self.with_vm_roots_frame();`
  immediately before `eval_sub_expr_vm_all_with_bindings`.

## 3. The mid-loop safepoint

`eval_loop.rs:3539`:

```rust
if index_gc::should_collect_midloop() {
    let mut midloop_roots: Vec<MettaValue> = collect_all_roots();   // env / tiers / promoted
    midloop_roots.extend_from_slice(root_set.roots());             // S/C/K + frame chain (incl. VM frames)
    index_gc::run_collection_if_triggered_midloop(&midloop_roots);
}
```

The root set is `collect_all_roots()` (registry: environment, tier caches,
`MettaState` output, deferred-env, …) **unioned with** the trampoline's own
`RootSet` (`root_set.roots()`), which walks the frame chain — and therefore every
nested `BytecodeVm` frame's execution stacks via §2. **This union is the asserted
complete mid-execution root set; ASAN must confirm it (a missed root → a value
freed mid-execution → heap-use-after-free).**

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
3. **Pass = 0 ASAN errors** (root completeness proven) AND conformance still
   483 / 221 / 40. Then flip `midloop_enabled()` default to on (or drop the env
   gate) + re-confirm + commit.
4. If an ASAN UAF appears: a live value class is unrooted — extend
   `collect_roots_into` / `with_vm_roots_frame` coverage; do not flip on.

## 8. Relation to the parallel collector (Inc 6b-2)

This same per-frame VM rooting is a prerequisite for the parallel collector: each
parked worker has its own nested trampoline/VM stack, and the rendezvous collector
must root every parked worker's frames. Proving single-threaded mid-loop rooting
complete (under ASAN) validates the per-frame rooting that the parallel collector
will reuse, frame-chain-per-worker.
