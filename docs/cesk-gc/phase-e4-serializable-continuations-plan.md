# Phase E4 — Serializable Continuations: Implementation Plan

> Source-grounded plan (Plan agent, 2026-06-09). Status of the **formal boundary: DONE + green**;
> the Rust capture/restore/checkpoint **implementation is MISSING** — that is all E4 must add.
> Children: #254 (capture/restore/checkpoint + reject-without-closure), #255 (restored-execution safety tests).

## What already exists vs. what E4 must add

| Artifact | State | Evidence |
|---|---|---|
| Rocq theorem (slice safety) | DONE, fully `Qed` | `formal/rocq/gc/SerializableContinuationSlice.v:27-81` |
| TLA spec + invariant `NoRestoredFutureTouchFreed` | DONE | `tla/SerializableContinuationSlice.tla:60-97` |
| 3 TLC discriminators (`_all` pass / `_missing_child` / `_missing_kont` fail) | DONE | `tla/MC_SerializableContinuationSlice_*.cfg`; run at `verify_cesk_gc_formal.sh:301-306` |
| Rocq run wired into harness | DONE | `verify_cesk_gc_formal.sh:150` |
| **Rust capture/restore/checkpoint code** | **MISSING** | no module in `src/` |
| **Source-coupling pins (proof ↔ source)** | **MISSING** | zero `Serializable` refs in source-coupling script |
| **Restore-execution tests** | **MISSING** | only E3 spine tests exist |

The proof is serializer-format-INDEPENDENT (intentional) → **no `.v` edit needed**; the obligation is to make
the implementation satisfy the premises (`Hseed`, `Hclosed`, `Freed ∩ Slice = ∅`) and PIN them via source-coupling.

## Genuine-CESK core
The captured slice **IS** `σ|_Reachable(⟨C, E_local, a_k⟩)`: the closure walk reuses the GC reachability fold
(`IndexArena::mark_from_roots_with` index_arena.rs:1224 + `IndexHeap::child_addrs_for_mark` index_heap.rs:911),
so the serialized Addr set == the set the collector would keep live == `Reach(Seed)` in the proof.

## API (new module `src/backend/eval/cesk/continuation_slice.rs`, `#[cfg(feature = "index-gc")]`)

- `SerializedContinuationSlice { control: Vec<SlotRef>, env_local: Vec<(VarId,SlotRef)>, kont: Vec<SlotRef>,
  nodes: Vec<(u32 old_raw, SerNode)>, children: Vec<Vec<u32>>, bytes: Vec<Box<str>>, spans: Vec<SerSpan>,
  spaces: Vec<SerSpace>, depth: u32, total_reductions: u64 }` (serde+postcard; deps already present).
  - `SlotRef` = old raw u32 Addr **+ the 4 handle flag bits** (so `MettaValue::from_addr(addr,flags)` metta_value.rs:558 reconstructs the exact handle); inline scalars (Bool/i48-Long/Unit/Empty) carried by value.
  - `SerNode` = serde mirror of `Node` (index_node.rs:81; 18 Copy variants) with ByteRef/ChildRef/SpanRef → dense indices into bytes/children/spans, child MettaValues → SlotRef/inline.
  - Closed by construction: `nodes` == `Reach(seeds)` under `child_addrs_for_mark` edges == proof's `Hclosed`.
- `capture_slice(control, env_local, kont, depth, total_reductions) -> SerializedContinuationSlice`
  1. seed-extract via `MettaValue::as_arena_addr` (metta_value.rs:544) → record each seed `(raw,flags)` into its vector (pins the proof's 3 seed sets `SerializedSeed` .v:22).
  2. closure walk — reuse the GC worklist (prefer a non-bit-setting variant `IndexHeap::collect_slice`/`reachable_closure` next to `child_addrs_for_mark`:911 / `mark`:967), reading `arena.get(addr)` (721) + draining side columns (children 923 / bytes / spans / Space via `space_handle(id).collect_gc_values` 938).
- `restore_slice(&slice) -> Result<RestoredSuspension, SliceError>` — **discriminators live HERE** (#254 "reject without closure"):
  - **D-KONT** (missing-K-root): a suspended K with frames but empty/incomplete `kont` → `Err(MissingKontRoot)` (== TLA `_missing_kont.cfg`).
  - **D-CHILD** (missing-reachable-child): some node's child Addr absent from `nodes` → `Err(MissingReachableChild)` (== TLA `_missing_child.cfg`; runtime image of `Hclosed`).
  - rehydrate: re-intern every node children-before-parents via `IndexHeap::alloc_*` (382-526) building `remap: HashMap<u32 old_raw, Addr new>`; rebuild C/E_local/K via `from_addr(remap[old], flags)`; return `RestoredSuspension { work_stack, continuations, depth, total_reductions }` → `resume_trampoline_inner` (eval_loop.rs:3988).
- `checkpoint(&slice) -> Vec<u8>` = `postcard::to_allocvec`; `restore_from_bytes(&[u8])` = `postcard::from_bytes` then `restore_slice`. Full pipeline capture→serialize→deserialize→restore→resume (#255).

## Addr stability: REMAP to FRESH Addrs on restore (do NOT reuse source Addrs)
`Addr=(seg<<18)|off` is segment/offset-encoded (index_arena.rs:104); a target arena's cur_seg/free-list is
independent. Reusing raw Addrs would alias live nodes → the UAF the proof forbids. Fresh re-intern + total
remap over the closure is the only sound choice and keeps `ConcurrentBumpFreshOnly` intact. Capture stores raw
Addrs as **opaque keys**; only restore binds them to a concrete arena → "serializable BY CONSTRUCTION".

## Other source edits
- `index_heap.rs`: add `reachable_closure(&self,&[Addr])->Vec<Addr>` + `emit_node(&self,Addr)` after `child_addrs_for_mark`:961 / `mark`:970 (both `#[cfg(feature="index-gc")]`).
- `metta_value.rs`: add `pub(crate) fn addr_flags(&self)->usize { self.tagged & 0xF }` near 551.
- `rholang_integration.rs`: at `run_state_async`:402 add `run_state_async_resumable` → `RunOutcome ∈ {Completed(MettaState), Suspended(SerializedContinuationSlice)}` on `EvalOutcome::Yielded` (eval_loop.rs:3965) + a "ship" policy; `resume_shipped(buf, &MettaState)` → `restore_from_bytes` → `resume_trampoline_inner` → drain into `MettaState.output` (430/459). Additive, `#[cfg(all(feature="async",feature="index-gc"))]`. A captured slice is self-rooting (owns copied Node bytes) → no SafepointRootHandle across the ship boundary.
- `reductions.rs`: `SuspendedEval`:90 ↔ slice bridge `to_slice`/`from_restored` (extract C/E_local/K seeds via the existing `Continuation::collect_values`/`RootSet::collect_from_*` state.rs:191/371 — same root collection as the GC → seed completeness == root completeness).
- `mod.rs`: register `pub mod continuation_slice;` after 42 + re-export after 69 (both `#[cfg(feature="index-gc")]`).

## Source-coupling pins to ADD (the #254 "complete" gate) — mirror the E3 block (script 783-824)
- seed set: `line_no continuation_slice.rs "control:"/"env_local:"/"kont:"`; assert_after_before `capture_slice` folds all three through `as_arena_addr`.
- closure completeness: assert_after_before that `capture_slice`/`collect_slice` calls `reachable_closure` and `reachable_closure` uses `child_addrs_for_mark` (slice == σ|_Reachable).
- reject-without-closure: `assert_after_before continuation_slice.rs "fn restore_slice" "MissingKontRoot" "MissingReachableChild"` + pin both `SliceError` variants returned.
- proof/theorem names: `line_no verify_cesk_gc_formal.sh 'run_rocq ".../SerializableContinuationSlice.v"'`; `line_no .v "restored_future_touch_not_freed"/"SerializedSeed"/"reachable_store_in_serialized_slice"`.
- TLA discriminators: `line_no verify_cesk_gc_formal.sh 'run_tlc "serializable_continuation_slice_missing_kont"'` etc (already at 301-306 — pin so they can't silently drop). Optional: add `_missing_control.cfg` (`IncludeControl=FALSE`).

## Tests (#255, `#[cfg(all(test, feature="index-gc"))]`, flip index mode like index_node.rs:163)
1. Round-trip equivalence: drive a multi-step eval to a yield (small `METTATRON_REDUCTION_BUDGET` reductions.rs:48); arm A = `resume_trampoline_inner` direct; arm B = `capture→checkpoint→restore_from_bytes→resume`; assert identical result multiset. (Mirror E3 `trampoline_fanout_production_spine_persists_and_resolves_process_amb` types.rs:3711.)
2. D-CHILD: delete a non-seed referenced node → `Err(MissingReachableChild)`.
3. D-KONT: clear `slice.kont` on a non-trivial spine → `Err(MissingKontRoot)`.
4. Freed-address non-touch (`restored_future_touch_not_freed`): free/recycle unrelated Addrs in another arena, restore (fresh remap) + resume, assert no read of a freed slot (empirically via ASAN §ladder-10).
5. Closure == GC-reachable: assert `capture_slice(seeds).nodes` key-set == `IndexHeap::mark`-reached set from the same seeds (runtime analog of `reachable_store_in_serialized_slice`).

## Verification ladder (capped/foreground, tee long runs)
1. capped index build `--features index-gc` (slab unchanged — module is cfg-walled).
2. focused `cargo nextest run --release --features index-gc continuation_slice`.
3. `scripts/verify_cesk_gc_formal.sh` (Rocq 150 + 3 TLC 301-306 stay green).
4. `scripts/verify_cesk_gc_source_coupling.sh` (now includes the E4 pin block).
5. `scripts/verify_cesk_gc_proof_hygiene.sh` (.v stays Qed).
6. `scripts/verify_cesk_gc_tlc_hygiene.sh` (negative E4 runs carry discriminator patterns).
7. `scripts/a5_greenwall.sh` — slab 4312/0 byte-identity AND index 483/221/40 byte-identical, cycles>0.
8. ≥20-run determinism of round-trip (remap is content-addressed, no Addr-identity leak).
9. mmverify "Correct proof".
10. ASAN (restore re-interns + resume executes over σ) with forced GC cycles → 0 UAF (empirical discharge of `restored_future_touch_not_freed`).
