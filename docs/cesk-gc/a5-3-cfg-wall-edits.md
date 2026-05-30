# A5.3 — cfg-wall the RootProvider impls + registrations to slab (verified edit guide)

Verified against HEAD ~`98ffe05` (A5.2) by a Plan agent. A5.3 puts `#[cfg(not(feature = "index-gc"))]`
on each `RootProvider` impl + its registration; KEEPS every inherent collector body UNCONDITIONAL
(the index build reads them by name via `collect_global_anchors`). After A5.3 the index registry is
EMPTY (still exists; A5.5 removes it); slab byte-identical.

## LOAD-BEARING result: NO gap. `collect_global_anchors` (roots.rs:274-295) + `collect_persistent_roots`
(roots.rs:351-363) already cover all 7 body-bearing providers in NEW (env via env0.collect_roots_into:358;
tiered:280 / space:281 / memo:282 / bytecode:283 / compiler-atom:284; MettaState via KEPT
`collect_driver_program_roots`). The 3 parallel providers contribute ∅ in the index regime (gate
`gc_mode_is_index() && !worker_ever_spawned() && active==0`; both reg sites + CurrentIterScope sites are
downstream of `note_worker_spawned()`; FANOUT_DEPTH=0 never spawns). ⇒ pure cfg-wall; OLD shrinks
monotonically; `OLD ⊆ NEW ∪ KEPT` preserved + non-vacuous (collect_all_roots term untouched — that's A5.5).

## Verified inventory (file:line at HEAD 98ffe05)
| # | Provider | impl | registration | KEEP body (unconditional) |
|---|---|---|---|---|
| 1 | GenericEnvironmentShared<MettaValue> | core.rs:2268 | **gc_allocator.rs:4236** in `try_register_env_roots` (NOT core.rs:2200/2250 — doc was wrong) | collect_roots_into core.rs:2168 |
| 2 | MettaStateGcRoots | metta_state.rs:25 | metta_state.rs:122-124 in `from_parts` | collect_driver_program_roots :233 |
| 3 | BytecodeCacheRoots | cache.rs:242 | cache.rs:303 in `ensure_bytecode_cache_roots_registered` get_or_init | collect_bytecode_cache_roots :255 |
| 4 | SpaceRegistryRoots | space_registry.rs:165 | :182 in `ensure_space_registry_roots_registered` | collect_all_gc_values :146 |
| 5 | MemoCacheRoots | memo_cache.rs:247 | :264 in `ensure_memo_cache_roots_registered` | collect_all_values :198 |
| 6 | TieredCacheRoots | tiered_cache.rs:1929 | :1946 in `ensure_tiered_cache_roots_registered` | collect_roots_into :1104 |
| 7 | CompilerAtomRoots | compiler/iterative.rs:49 | :85 in `ensure_compiler_atom_roots_registered` | collect_compiler_atom_roots :62 |
| 8 | ParallelDispatchRootProvider | types.rs:298 | eval_loop.rs:2534 | (none — parallel) |
| 9 | ParallelCollapseRootProvider | types.rs:373 | eval_loop.rs:3002 | (none — parallel) |
| 10 | CurrentIterRootProvider | current_iter_root.rs:88 | :128 | (none — whole file → slab) |

## Edit patterns
- **MettaState (2)**: cfg impl@25; wrap the `let provider…; register_root_provider(&provider);` (122-124) in
  `#[cfg(not(feature="index-gc"))] { … }`; SPLIT import metta_state.rs:6 → keep `GcFactory` uncond,
  `{register_root_provider, RootProvider}` under cfg(not). KEEP struct + fields + collect_driver_program_roots.
- **5 OnceLock caches (3-7) — TRAP**: `ensure_X_registered` is CALLED by collect_global_anchors in INDEX, so
  its SIGNATURE must survive in index. Use a TWO-ARM cfg: `#[cfg(not(index-gc))] fn ensure_X(){ get_or_init{
  Arc::new(XRoots) as Arc<dyn RootProvider>; register_root_provider(&p); p } }` + `#[cfg(feature="index-gc")]
  #[inline] fn ensure_X(){}`. ALSO cfg(not) on: the `impl RootProvider for XRoots`, the `static X_ROOT_PROVIDER`,
  AND `struct XRoots;` (else index dead_code). SPLIT each import (register_root_provider/RootProvider → cfg(not);
  keep MettaValue/ValueView/etc). KEEP the collector body + cache accessors (global_*) unconditional.
  Lines: cache.rs struct@240/impl@242/static@294/fn@300/import@28; space_registry struct@163/impl@165/static@173/
  fn@179/import@31; memo_cache struct@245/impl@247/static@255/fn@261/import@225(whole line cfg); tiered_cache
  struct@1927/impl@1929/static@1937/fn@1943/import@35(whole line cfg); iterative struct@47/impl@49/static@76/
  fn@82(`fn` not `pub fn`)/import@20.
- **env-fork (1)**: TWO-ARM cfg on `try_register_env_roots` (gc_allocator.rs:4208) — slab arm = current body;
  index arm = `{ /* E₀ read structurally */ }` with `_shared` param. cfg impl@core.rs:2268; SPLIT core.rs:89
  import → `try_register_env_roots` uncond, `RootProvider` under cfg(not). The 5 callers (core.rs 634/882/953/
  1404/1931) need NO edit. KEEP collect_roots_into@2168.
- **parallel (8,9)**: cfg(not) impls types.rs:298/373; cfg(not) the `register_root_provider(...)` CALL at
  eval_loop.rs:2534/3002. Do NOT wall the `let root_provider = Arc::new(...)` bindings (2525/2994 — moved into
  `_root_provider_arc` handle field, used in both builds) or the structs (types.rs:269/364).
- **CurrentIter (10)**: cfg(not) the `mod current_iter_root;` decl at trampoline/mod.rs:40 (whole file out of
  index). cfg(not) the 2 `CurrentIterScope::enter` call sites eval_loop.rs:2427-2428 / 2906 (the `branch_expr`/
  `item_expr` stay used by the rest of each worker closure).

## Order: MettaState first (§2.3 probe — if oracle survives MettaState-unregistered, the rest is a fortiori safe),
then 5 caches, then env-fork, then 2 parallel, then CurrentIter. ONE commit.

## Green-wall + ASAN: `scripts/a5_greenwall.sh A53 --with-oracle`. Expect slab 4324/index 4176 UNCHANGED
(A5.3 walls no #[cfg(test)] → no test delta), conformance 483/0, 0 oracle panics. **ASAN BOTH builds**
(A5.3 touches SLAB registration → slab ASAN MANDATORY — the slab-provider-drop UAF). INDEX ASAN = M11-bisimilarity-pt
conformance (NOT stress_multidir-CLI — pre-existing driver-C gap, A5.4). 0-new-warnings via clean `cargo clippy`
(NOT rust-analyzer — STALE-cache caveat); watch the 7 import-splits + the walled structs.

## RISKS: (1) #1 = a `#[cfg(feature="index-gc")]` INVERSION on a slab item silently drops a slab provider →
slab UAF (mitigate: slab ASAN + slab oracle + 4324 byte-identical). (2) the OnceLock two-arm-cfg trap (don't
wall the signature/callers). (3) don't over-wall the parallel `root_provider` bindings; DO wall the cache structs.
(4) the import-split sweep is the error-prone mechanical part. (5) leave collect_all_roots/ROOT_REGISTRY/
register_root_provider FN in place (A5.5).
