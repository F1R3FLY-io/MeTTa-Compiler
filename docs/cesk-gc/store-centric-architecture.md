# Store-Centric CESK/SECK Garbage Collector — Target Architecture & Migration

> **AUTHORITATIVE DESIGN** (2026-05-28). Supersedes the allocation strategy of
> `~/.claude/plans/would-it-be-possible-resilient-gosling.md` — its "mode-aware
> `GcFactory` / `GC_MODE`-flag retrofit" + "keep the registry" stance are **REJECTED**
> by the user. Its collector *strategy* (non-moving mark-sweep at quiescence over one
> global segmented arena) is **retained** (never the disagreement). Source-verified by
> a Plan agent. pgmcp work-item `gc-clean-room-migration-…-13074ec1`.

## Philosophy (Van Horn & Might AAM; Might & Shivers abstract-GC) — honor exactly
CESK state `ς = ⟨C, E, K, σ⟩`. **The store σ IS the heap** (`σ: Addr → Value`; addresses
are indices, never raw pointers). **The environment is a layer of indirection**
(`E: Var → Addr`, never `Var → Value`). **Allocation is a store operation**
(`alloc(σ,v) → (Addr, σ')`). **GC is a pure algebraic function of ς**:
`Ψ(ς) = T(C,E) ∪ T(K)` (addresses *touched* by control/env/kont), `Reachable = lfp` over
σ's address-graph, `GC(⟨C,E,K,σ⟩) = ⟨C,E,K, σ|_Reachable⟩`. **NO manual root registry** —
external live surfaces (global env, tier caches, deferred-drop) are part of Ψ, computed
*structurally*. **Liveness layers** are separate Addr-keyed maps (store, mark-bitmap,
region, opt. generation) decoupled from value bytes.

```
  ς = ⟨ C , E , K , σ ⟩            MettaValue == Addr (8-byte Copy; scalars NaN-boxed inline)
  C  work_stack: Vec<WorkItem>     (Addr-bearing)
  E  bindings:  Var → Addr         (HashMap<Var, MettaValue>; MettaValue=Addr ⇒ Var→Addr)
  K  continuations: Vec<Cont>      (Addr-bearing)
  σ  IndexHeap: Addr → Node        THE HEAP (segmented, non-moving)
   Var ─E─▶ Addr ─σ.node─▶ Node ─σ.side[seg]─▶ children:[Addr]/bytes/span
                  │  (GC reasons ONLY over Addr-keyed maps, never value bytes)
        mark: Addr→bit (AtomicU64 bitmap) · region: Addr→seg (Addr high bits) · [gen: Addr→g]
```

## Verified ground truth (file:line)
- **σ exists & is reused:** `index_arena.rs` (`IndexArena<N>` :221, `alloc` :279, `get` :303,
  `mark`/bitmap :319/:129, `sweep`/`sweep_with` :354/:361, `mark_from_roots_with` :409;
  `Addr=(seg<<18)|off` :41,:76). `index_heap.rs` (`IndexHeap` :74, `view_at` :299,
  `materialize_inner` :345, `mark` :379, `sweep` :397, `IndexFactory: MettaValueFactory` :453,
  `IndexHeapStore: Store` :614). `index_node.rs` (`Node` Copy mirror).
- **Ψ (trampoline part) exists & is reused:** `state.rs:182` (`collect_gc_roots`), `roots.rs`,
  inline safepoint enumeration `eval_loop.rs:3386–3414`.
- **CRUCIAL: `GenericEnvironment<V,F>` is ALREADY generic** (`core.rs:486`, `factory: F` :495);
  the whole `environment/*.rs` tree is already `impl<V,F> where F: MettaValueFactory<V>`. The
  slab `GcFactory` is welded at **exactly 4 narrow points**, NOT 1082:
  (1) `EvalContext::factory(&self) -> &GcFactory` concrete return `context.rs:33`;
  (2) `type Environment = GenericEnvironment<MettaValue, GcFactory>` `engine.rs:30`;
  (3) `type MettaEnvironment = …GcFactory` `context.rs:147`;
  (4) ~247 `&GcFactory`-typed params in `engine.rs` (`:56,66,119,…`).
  The 1082 `ctx.factory()` sites call *trait* methods → DON'T change.
- **Registry is a redundant async side-channel:** `ROOT_REGISTRY` `gc_allocator.rs:3598`,
  `register_root_provider` :3608, `collect_all_roots` :3631, 10 impls (`core.rs:2160`,
  `tiered_cache.rs:1929`, `metta_state.rs:25`, `space_registry.rs:165`, `memo_cache.rs:247`,
  `compiler/iterative.rs:49`, `current_iter_root.rs:88`, `cache.rs:242`, `types.rs:298,373`).
  The safepoint ALREADY enumerates ~all of Ψ inline (`eval_loop.rs:3386`); registry is the 2nd path.
- **State is a value-by-id** (`MettaValueInner::State(u64)` is an id, `metta_value.rs:647`; cells in
  `shared.states: RwLock<HashMap<u64,V>>` `core.rs:331`, mutated by `change_state` `mutable_state.rs:51`)
  → the *map of values* is a root; NO embedded old→young handle in σ ⇒ **no write barrier** (full
  transitive mark from all roots incl. the State map each cycle).
- **Bridge (transient):** `GC_MODE` `metta_value.rs:508`; mode-branches `view` :1117/`inner_ref` :972/
  `inner_ptr` :1266; `INNER_SHADOW` :863. **Slab value model:** `&'static MettaValueInner` raw ptr +
  `unsafe impl Send/Sync ×4` `:711–723`. **Epoch/ABA:** `gc_allocator.rs:346`, `VALUE_HASH_CACHE` epoch
  invalidation `metta_value.rs:116,126`. **Live collector is concurrent-snapshot, NOT quiescent**
  (`context.rs:75`; `worker_cooperative_safepoint` async-register-only `eval_loop.rs:188`).
- **Global-arena constraint** (dispositive): `docs/post-mortems/BATCH_PARALLELISM_SEGFAULTS.md:38` —
  per-machine/concurrent arenas corrupted jemalloc; ONE global σ + per-thread TLABs claiming whole
  segments (bulk, not per-object malloc).

## Design decision A — thread σ via generic factory (option i). REJECT dyn-shim & mode-GcFactory.
`EvalContext` gains assoc `type Factory`; `factory()` returns `&Self::Factory`; `engine.rs` helpers
become `<F: MettaValueFactory<MettaValue>>`; the 4 weld points are the swap. `IndexFactory` already
IS the store's alloc interface (each ctor → σ). `GcFactory` dissolves into one legacy `MettaValueFactory`
impl behind `--features legacy-slab-gc`. NO per-alloc vtable (factory is Copy ZST/`&'static`-sized). NO
permanent global mode flag. The 1082 call sites are unchanged (trait methods). End-state: `MettaValue=Addr`
native (scalars NaN-boxed orthogonally); slab raw-pointer model is the legacy.

## Re-planned increment ladder (replaces prior 3–9). Green wall every rung: nextest ~4312 / mtt 483 / M11-pt 221 / M11-he 40 / no Slab regression; commit only on user request.
- **2b** JIT/bytecode index decode — DONE this session (gate T2/T3 off under index → VM(T1) fallback;
  entry-boundary pack/unpack mode-aware). NOTE: finish the partial `JitContext.arena` seam
  (`jit/types/context.rs:354` + `from_addr` arms `value.rs:384,405`) — it's PARTIAL, not absent.
- **3** A/B differential infra: `--gc={slab|index}` / `MTT_GC` startup flag is now an
  assertion/reporter for the compile-time store, not a runtime selector;
  `scripts/ab_gc_diff.sh` compares a default-index binary against the explicit
  legacy slab opt-out.
- **4** Flip **sequential** to σ via **generic threading (option i)**: `EvalContext::factory`→assoc `type Factory` (`context.rs:33`); `engine.rs` `&GcFactory`→`<F>` (~247 sigs); `MettaEnvironment` alias (`context.rs:147`); `StaticEvalContext.store`/`SessionContext`→`IndexHeapStore`. **Dissolves GcFactory hardcoding.** Gate: green wall + 20-run PLN/mmverify stability + ASAN(index) + Welch(seq) ≠ TERRIBLE. Rollback: alias back to `SlabStore` (1 line).
- **5** Flip **parallel** to σ + lock-free TLABs (refine `index_heap.rs:426` RwLock→TLAB claiming segments `index_arena.rs:266`); parallel-dispatch providers (`types.rs:298,373`)→Ψ. Gate: parallel-stability + ASAN + Welch(PLN {wall,RSS}) ≠ TERRIBLE.
- **6** **Algebraic σ|_Reachable as SOLE collector**: wire `IndexHeap::mark(Ψ)`+`sweep` at safepoint (`eval_loop.rs:3454`); add guard-drop/park to `worker_cooperative_safepoint` (true quiescence); **fold the 10 providers into structural Ψ & DELETE `ROOT_REGISTRY`/`register_root_provider`/`collect_all_roots`**; **DELETE epoch/ABA/128-bit-CAS/`VALUE_HASH_CACHE` epoch-invalidation**; rehome watermark+4-level backpressure into safepoint; re-key `mork_convert` ground-cache to Addr-bitmap; wire `INNER_SHADOW`/hash-cons clear to real sweep. **Dissolves registry (#1) + epoch/ABA (#4).** Gate: **Welch ACCEPT all primary endpoints post-PGO** + OS-RSS-on-release + new TLA+ `StoreCentricGC` green.
- **7** Feature-gate slab behind `--no-default-features --features legacy-slab-gc`
  (`#[cfg]` on `GcFactory`/`SlabAllocator`/`SlabStore`); default build = index only.
  Gate: green wall (default index) + explicit legacy slab opt-out green.
- **8** **DELETE the bridge**: remove `GC_MODE`/`gc_mode_is_index`/mode-branches/`INNER_SHADOW`; collapse `view`/`inner_ref`/`inner_ptr` to one (index) body; auto-derive `Send/Sync`; retire the `&'static MettaValueInner` raw-ptr model. Gate: green wall + no `gc_mode_is_index` refs remain.
- **G** (opt-in) generational nursery over σ (Addr→gen map + remembered-set barrier). **P** (opt-in) parallel marking (work-stealing into the existing `AtomicU64` bitmap — no rep change).

Sequencing rationale: value model flips first (4–5, σ becomes the heap) WHILE the ported slab collector still runs; collector strategy swaps to algebraic σ|_Reachable next (6, registry dissolved here); bridge removed last (8) after slab is safely feature-gated (7).

## Formal model (Inc 6): new `tla/StoreCentricGC.tla`. Ψ is a *derived state expression* (not a registry var) ⇒ NoLostObjects = completeness of ONE structural mark; registry-desync bug class UNMODELABLE. + NoUseAfterFree, SegmentReleaseSafety, QuiescenceInvariant (NEW — re-derives data-race-freedom under true quiescence), NoConcurrentFree (proves ABA-free ⇒ epochs deletable). TOCTOU-UAF + page-UAF unmodelable; non-moving ⇒ no RelocationCorrectness ⇒ determinism by construction.

## REUSED (don't rewrite): index_arena, index_heap, index_node, SeckState/RootSet, Store trait, the safepoint Ψ-enumeration, the vetted non-moving-mark-sweep-at-quiescence strategy, the no-`_`-wildcard collect_values walkers.
## DISSOLVED (must not survive end-state; with the inc that does it): GcFactory/SlabAllocator/SlabStore (generic-thread Inc 4 → feature-gate Inc 7); RootProvider registry (Inc 6); epoch/ABA (Inc 6); GC_MODE/INNER_SHADOW/mode-branches (Inc 8); `&'static` raw-ptr value model + 4 unsafe Send/Sync (Inc 8); gc_thread/gc_cron/gc_pool (Inc 6).
## NET-NEW: structural Ψ as sole root path (6); true-quiescence rendezvous (6); lock-free TLABs (5); `--gc`/`MTT_GC` (3) + `--features legacy-slab-gc` (7); StoreCentricGC.tla (6); Welch gating (4/6); opt-in gen map (G) / parallel mark (P).
