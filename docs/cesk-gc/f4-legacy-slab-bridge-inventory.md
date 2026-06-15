# Phase F4 — Legacy Slab Bridge: Live-Surface Inventory & Proof Map

**Tracker:** pgmcp #478 (inventory, this document) → #432 (deletion) → epic #49 (Phase F) → root #30.
**Baseline:** branch `feature/petta-semantics`, HEAD `3bab0dc2` (formal wall green: 125 Rocq + 36 Lean + 379 TLC + source-coupling + hygiene).
**Goal of F4:** delete the legacy slab collector so `index-gc` is the only store, with `rg = 0` for the agreed *live-code* patterns (docs may retain explicitly-historical mentions). Done proof-first, green-at-each-step.

## How the erasure proofs license each deletion

The base model `formal/rocq/gc/DefaultStoreSelection.v` defines `Store ∈ {IndexStore, SlabStore}` and `active_store(features)`:
`has_index_gc ∧ ¬legacy ⇒ IndexStore`; `¬index ∧ legacy ⇒ SlabStore`; otherwise `None` (Cargo-additivity safety). Each `*Erasure.v`
proof discharges the pair **(erase)** `active_store = IndexStore ⇒ legacy_effect = None` and **(requires-slab)** `legacy_effect = Some _ ⇒ active_store = SlabStore`. That pair is the formal license that deleting a legacy branch under the index store is behaviour-preserving.

## Inventory & proof map (verified at HEAD)

Reproducible commands run from repo root; the *default* build is `index-gc`.

| # | Surface | rg command | Class | Discharging proof |
|---|---------|-----------|-------|-------------------|
| S1 | `gc_thread` | `rg -n 'gc_thread\|GcThread\|GLOBAL_GC_THREAD' src -g'*.rs'` | **legacy-only, DEAD even in slab** (0 live callers; superseded by `AdaptiveGcPool`) | `DedicatedSingleRegime.v` (`dedicated_enabled_blocks_legacy_requests`, `dedicated_request_has_driver`, `no_driverless_request_under_dedicated`) + `GcPoolErasure.v` |
| S2/S3 | `gc_pool` / `GcPool` | `rg -n 'gc_pool\|GcPool\|AdaptiveGcPool' src -g'*.rs'` | legacy-only module + `cfg(not index-gc)` call sites in shared files (`gc_allocator.rs`, `gc_cron.rs`, `work_pool.rs:1485`, `diagnostics.rs:890/942`) | `GcPoolErasure.v` (`index_selection_erases_legacy_pool_effects`, `default_index_erases_legacy_pool_effects`, `emitted_pool_effect_requires_slab`) |
| S4 | `current_iter_root` | `rg -n 'current_iter_root\|CurrentIterRootProvider' src -g'*.rs'` | legacy-only (module gated `trampoline/mod.rs:44`; call sites `eval_loop.rs:2776,3658` gated) | `RootDiscoveryErasure.v` (`legacy_only_value_not_index_collector_root`, `future_touch_survives_without_legacy_discovery`) |
| S5 | `RootProvider` / `ROOT_REGISTRY` / `root_registry` | `rg -n 'RootProvider\|ROOT_REGISTRY\|root_registry\|register_root_provider' src -g'*.rs'` | legacy-only: trait + registry core (`gc_allocator.rs:5501-5610`) + `impl RootProvider` blocks (cache.rs, space_registry.rs, memo_cache.rs, compiler/iterative.rs, tiered_cache.rs, environment/core.rs:2311, types.rs:376/493, metta_state.rs:27), all `cfg(not index-gc)` | `RootDiscoveryErasure.v` (+ `RegistryIsolation.v`) |
| S6 | cfg seam / `GC_MODE` | `rg -n 'GC_MODE' src` → **0**; seam = `src/lib.rs:5-15` (`compile_error!` mutual-exclusion) | seam removed LAST | `CfgGuardErasure.v`, `RuntimeModeErasure.v` (`runtime_erasure_complete`, `runtime_request_cannot_switch_store`) |
| S7 | arena/inner-ptr decode | `rg -n 'arena_addr\|from_inner_ptr\|inner_ptr' src -g'*.rs'` | cfg-split decode; legacy slab-ptr branch deletable | `ArenaAddrDecodeErasure.v`, `InnerPtrDecodeErasure.v` |
| S8 | `GcFactory` / `SlabFactory` / `SlabAllocator` | `rg -n 'GcFactory\|SlabFactory\|SlabAllocator' src -g'*.rs'` | `cfg(not index-gc)` type aliases (`ActiveFactory=GcFactory`, `mod.rs:82`) + JIT factory construction | `JitValueCreationStoreSelection.v`, `JitTypeOpsStoreSelection.v`, `JitLongBoxStoreSelection.v`, `JitIsFunctionPointerDecode.v`, `JitPayloadConversionStorePolicy.v` |
| S5b | `gc_cron` legacy producer | `rg -n 'request_gc\|maybe_async_gc' src/backend/models/gc_cron.rs` | **shared module**: legacy producer deletable, `spawn_gc_cron`/`MonitorState`/`GcCronSingleton`/`cron_work_pool` KEPT | `CronProducerErasure.v` (`index_selection_erases_legacy_cron_request`, `monitor_return_preserved_by_cron_erasure`, `legacy_cron_request_requires_slab_and_pressure`) |

### `frame_chain` — RESOLVED (was mis-classified): legacy-only, live-in-legacy, deletes with the un-gating sequence

`src/backend/eval/frame_chain.rs` (`EvalFrameGuard`, `FrameLabel`, `collect_frame_chain_roots`) is **legacy-slab-only**, NOT shared. The whole module is `#[cfg(not(feature="index-gc"))]` (`eval/mod.rs:18`, comment: *"The legacy slab opt-out build compiles + uses frame_chain verbatim. F4 … physically deletes the file once the slab build itself stops needing it"*). Because the index build excludes the module, **every** use of `EvalFrameGuard`/`FrameLabel`/`collect_frame_chain_roots` (in `eval_loop.rs:259/2668/3250/3435/3537/4164/4440`, `vm/mod.rs:1288…4383`, `expr_vec_frame.rs:75`, `trampoline/types.rs:31`, `vm/tests.rs:8517…`) is itself `cfg(not index-gc)`-gated — the index collector carries parent-class roots structurally via the K-spine instead. So `frame_chain` is part of the deletable legacy surface (root-discovery cluster, guard `RootDiscoveryErasure.v`), but — like `RootProvider`, `gc_pool`, and the slab allocator core — it is **actively used by the legacy build**, so it can only be deleted once the legacy build is decommissioned (see R2). The "open R7 contradiction" was a misreading of cfg-gated call sites; **R7 is resolved**.

### Genuinely shared (KEEP) — do NOT touch in F4

- **`gc_sweep_epoch` / `bump_gc_sweep_epoch`** — shared-core, exported in the **non-cfg'd** common arm (`mod.rs:23`), called by `index_heap.rs:2646/2843/4544`, `metta_value.rs`, `dispatch_hints.rs`, `mork_convert.rs`; proven *kept* by `EpochProtectedCaches.v`, `OperatorCacheEpoch.v`. The "epoch/ABA" F4 target is ONLY the legacy slab snapshot-epoch ABA guard (lived in `gc_thread.rs`, removed by R1) — never bare `gc_sweep_epoch`.
- **`gc_allocator` / `gc_cron` shared core** — these modules compile in both modes (`mod.rs:3-4`, no cfg). KEEP: `spawn_gc_cron`, `cron_work_pool`, `GcCronSingleton`, `CronHandle`, `MonitorState`, `global_gc_cron`, `init_global_allocator`, `is_gc_disabled`, `is_gc_requested`, `gc_sweep_epoch`, `collect_safepoint_roots`, `register_temporary_roots`, `SafepointRootHandle`, `try_register_env_roots`. Delete only the `cfg(not index-gc)` remainder (slab allocator core, legacy cron producer).

**New proof obligations blocking early rungs: none.** The JIT store-policy gaps were already closed (`JitTypeOpsStoreSelection`, `JitPayloadConversionStorePolicy`, hybrid-arena decode coupling — commits `397a92ef`/`764158d9`/`3bab0dc2`).

## Deletion-rung sequence (#432)

**Key constraint (source-verified):** `frame_chain`, `RootProvider`/registry, `gc_pool`, and the slab-allocator core are `cfg(not index-gc)` AND *actively used by the legacy build*, so none can be deleted while a compilable `legacy-slab-gc` build still selects them. Since **F1 (index-vs-slab A/B) is already accepted/done**, the legacy build has no remaining purpose. Therefore the legacy build is **decommissioned at R2** (not last): removing the feature turns every `cfg(not index-gc)` block into permanently-excluded dead code, after which each surface deletes safely (un-gating its `cfg(index-gc)` sibling to unconditional) while the single remaining **index build + formal wall stay green**. This supersedes the earlier "feature-last / both-builds-green" ordering, which is not viable for live-in-legacy surfaces.

**Invariant per rung:** the default (index-gc) build + formal wall + index-gc nextest are green; **every rung edits the matching `verify_cesk_gc_source_coupling.sh` pins in the SAME commit** (pins reference exact lines/counts that vanish on deletion).

| Rung | Removes / changes | Guard | Build invariant |
|------|-------------------|-------|-----------------|
| **R1** ✅ | `gc_thread.rs` (whole, 7 tests); `mod.rs:7-8`; coupling pin (was 1649) | `DedicatedSingleRegime.v` | dead in BOTH builds (0 callers verified) — both builds green |
| **R2** | Remove `legacy-slab-gc` from `Cargo.toml`; rework `lib.rs:5-15` seam → single `#[cfg(not(feature="index-gc"))] compile_error!("index-gc is required …")`; update the 8 scripts + 2 docs that reference the feature; update the coupling script's legacy-feature assertions | `CfgGuardErasure.v`, `RuntimeModeErasure.v` | default build + wall green; `cargo build --no-default-features` fails with the clean "index-gc required" message (expected); legacy build intentionally decommissioned |
| R3 | `gc_pool.rs` + all `cfg(not index-gc)` pool call sites (`gc_allocator.rs`, `gc_cron.rs`, `work_pool.rs:1485`, `diagnostics.rs:890/942`); un-gate index siblings | `GcPoolErasure.v` | index build + wall green |
| R4 | `frame_chain.rs` + ALL its gated sites (`eval_loop.rs`, `vm/mod.rs`, `expr_vec_frame.rs`, `trampoline/types.rs`, `vm/tests.rs`) + `current_iter_root` module; un-gate K-spine siblings | `RootDiscoveryErasure.v` | index build + wall green |
| R5 | `RootProvider` trait/registry (`ROOT_REGISTRY`, `register_root_provider`, `collect_all_roots`) + all `impl RootProvider` blocks | `RootDiscoveryErasure.v` + `RegistryIsolation.v` | index build + wall green |
| R6 | `gc_cron` legacy `request_gc` producer + `maybe_async_gc` (keep shared monitor/singleton) | `CronProducerErasure.v` | index build + wall green |
| R7 | JIT slab arms + `ActiveFactory=GcFactory`/`ActiveStore=SlabStore` aliases + slab inner-ptr/arena-addr decode; un-gate index factory/decode | `Jit*StoreSelection.v`, `ArenaAddrDecodeErasure.v`, `InnerPtrDecodeErasure.v` | index build + wall green |
| R8 | `gc_allocator` slab-allocator core (`SlabAllocator`, `GcFactory`) + legacy epoch-ABA guard + remaining `cfg(not index-gc)` blocks + slab tests | relevant `*Erasure.v` | index build + wall green |
| **R-final** | Remove `index-gc` as a Cargo feature (make unconditional), delete the last `lib.rs` guard + ALL remaining `cfg(...index-gc...)` attributes; final `rg = 0` for the agreed patterns; rewrite ledger + coupling to index-only invariants | `CfgGuardErasure.v` | single unconditional build + wall green |

### Per-rung gate (R2 onward — only the index build remains)
```
systemd-run --user --scope -p MemoryMax=24G -p CPUQuota=600% bash scripts/verify_cesk_gc_formal.sh   # wall: proofs + source-coupling + hygiene
systemd-run --user --scope -p MemoryMax=24G cargo build  --release                                    # index-gc (default)
systemd-run --user --scope -p MemoryMax=24G cargo nextest run --release
rustfmt --edition 2021 --check <touched .rs> ; git diff --check
# R2 only — confirm the misconfiguration guard:
systemd-run --user --scope -p MemoryMax=8G cargo build --release --no-default-features 2>&1 | grep -q "index-gc is required"
```
(R1 additionally ran both feature builds, since at R1 the legacy build still existed and was the safety reference.)

## Risk register
- **R1 — RESOLVED:** `frame_chain` is legacy-only (whole module + all guard sites `cfg(not index-gc)`), not shared; deleted in R4 under `RootDiscoveryErasure.v`. The "unconditional call site" reading was wrong — the index build uses K-spine structural roots.
- **R2 — `gc_sweep_epoch` is shared-core (HIGH):** exported in the non-cfg'd common arm; proven kept by `EpochProtectedCaches.v`/`OperatorCacheEpoch.v`. The epoch/ABA F4 target is ONLY the legacy slab snapshot-epoch guard — never bare `gc_sweep_epoch`.
- **R3 — feature decommission ordering (HIGH):** live-in-legacy surfaces force the legacy build to be removed at R2 before they can be deleted; this is sound because F1 (the sole consumer needing both stores) is done. After R2, `cfg(not index-gc)` code is dead and deletes safely.
- **R4 — `gc_allocator`/`gc_cron` are shared (HIGH):** never wholesale delete; remove only the enumerated `cfg(not index-gc)` remainder; keep the shared symbols listed above.
- **R5 — coupling pins are deletion blockers (MECH, HIGH-freq):** edit each pin in the same commit as the source it references, or the wall's source-coupling stage goes red.
- **R6 — index build behaviour change = NONE:** every deleted block is `cfg(not index-gc)` (already excluded from the index build); un-gating a `cfg(index-gc)` sibling yields the exact code the index build already compiled. Verify via per-rung index `nextest`.
- **R7 — scripts/docs:** removing `legacy-slab-gc` (R2) breaks the legacy comparison arm in 8 scripts (`f1_welch_bench.sh`, `a5_asan_both.sh`, `ab_gc_diff.sh`, `f1_memory_effectiveness.sh`, `f3_default_store_soak.sh`, `a5_greenwall.sh`, `drlock_gate.sh`, `verify_cesk_gc_all.sh`) and 2 docs — updated in R2. No `examples/`/`benches/` Cargo target references the feature.
