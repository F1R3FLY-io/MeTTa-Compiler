# A5.5 — cfg-wall the registry CORE to slab; INDEX build becomes REGISTRY-FREE (genuine-CESK milestone)

Verified by a Plan agent against the A5.0–A5.4 tree. After A5.5 the index build no longer compiles
ROOT_REGISTRY / RootProvider / collect_all_roots — the index collector's roots are purely structural
(`collect_machine_roots`) ∪ KEPT (`collect_safepoint_roots`). Slab byte-identical (registry intact).

## ⚠ HEADLINE CATCH (the plan's A5.5 is INCOMPLETE — would NOT compile in index)
The 8 registry-core leaf symbols are not the whole story. The **slab GC-pool wrapper cluster** is compiled
in BOTH builds (runtime-inert in index via `gc_mode_is_index()` early-returns, but COMPILED) and calls the
walled leaf symbols → unconditional call to a `#[cfg(not(index-gc))]` symbol from index code = E0425. A5.5
MUST also wall these wrappers/calls (all slab-only, runtime-inert in index → zero index runtime-behavior change):
- `trigger_gc_cycle` (gc_allocator.rs ~4275) → calls collect_all_roots@4309. Wall WHOLE fn.
- `trigger_gc_cycle_via_pool` (~4326) → calls collect_all_roots@4329. Wall WHOLE fn. Callers maybe_quiescent_gc@3433 + maybe_async_gc@3519 → two-arm the CALL stmt (`#[cfg(not)] let result = trigger_gc_cycle_via_pool(); #[cfg(feature)] let result = false;`).
- `trace_surviving_set` (~5146, pub fn KEPT — compiled-reachable in index) → calls collect_all_roots_readonly@5152. Two-arm the CALL stmt (`#[cfg(feature)] let roots: Vec<MettaValue> = Vec::new();`).
- `trace_safepoint_live_set` (~3903, pub(crate) fn KEPT) → env-roots branch@3929-3932 calls collect_provider_roots_readonly. Two-arm ONLY the branch (`#[cfg(feature)] let (env_roots, env_complete): (Vec<MettaValue>, bool) = (Vec::new(), true);`).

## Prereq check (DONE by agent): index is clean of the leaf symbols EXCEPT the oracle OLD-terms + the 4 wrappers above.
ZERO `impl RootProvider` compiled in index (all 10 A5.3-walled) ⇒ the trait DEF itself walls cleanly. ZERO
`register_root_provider` calls in index. All other collect_all_roots refs = #[cfg(test)] or [doc].

## EDIT SEQUENCE (one commit)
1. **gc_allocator.rs leaf symbols** — prepend `#[cfg(not(feature = "index-gc"))]` to: `trait RootProvider`@3640,
   `static ROOT_REGISTRY`@3656, `fn root_registry`@3658, `pub fn register_root_provider`@3666,
   `pub fn collect_all_roots`@3689, `fn collect_all_roots_readonly`@3729, `fn collect_provider_roots_readonly`@3857.
   KEEP UNCONDITIONAL: SAFEPOINT_ROOTS@3774, safepoint_registry@3776, SafepointRootHandle+Drop@3784/3789,
   register_temporary_roots@3810, collect_safepoint_roots@3840 (A5.4 transport).
2. **gc_allocator.rs wrappers** (the catch §): wall trigger_gc_cycle@4275 + trigger_gc_cycle_via_pool@4326 (whole fns);
   two-arm the trigger_gc_cycle_via_pool() calls @3433/3519; two-arm the collect_all_roots_readonly() call @5152;
   two-arm the collect_provider_roots_readonly env-branch @3929-3932 (keep env_complete=true in index).
3. **gc_allocator.rs #[cfg(test)] registry tests** (~7090-7250) — wall the `#[test]` fns calling collect_all_roots/
   register_root_provider with `#[cfg(not(feature="index-gc"))]`. Index nextest drops by EXACTLY that count
   (tests-only delta — assert it in the green-wall).
4. **roots.rs quiescence oracle OLD** @383-384 — two-arm the initializer:
   `let mut old_vals = { #[cfg(not(feature="index-gc"))] { collect_all_roots() } #[cfg(feature="index-gc")] { Vec::new() } };`
   (block-expr, NOT attr-on-let — keeps old_vals defined; index OLD = result).
5. **eval_loop.rs midloop oracle OLD** @3630-3632 — `#[cfg(not(feature = "index-gc"))]` on the
   `for v in collect_all_roots() { old.push(...) }` loop (index OLD = root_set.roots()).
6. **models/mod.rs re-exports** @17-28 — split: move `collect_all_roots, register_root_provider, trigger_gc_cycle,
   RootProvider` into a `#[cfg(not(feature = "index-gc"))] pub use gc_allocator::{...};` arm. KEEP in common:
   collect_safepoint_roots, register_temporary_roots, SafepointRootHandle, maybe_quiescent_gc, try_register_env_roots, etc.

## Oracle GREEN + NON-VACUOUS (load-bearing)
GREEN: index `collect_all_roots()` returned only (0 providers) ∪ collect_safepoint_roots = ⊆ KEPT already → dropping
it only SHRINKS OLD → `OLD ⊆ NEW∪KEPT` preserved by monotonicity. NON-VACUOUS: index midloop OLD = `root_set.roots()`
still ⊇ S∪C∪K + 4 thread-local caches + deferred (A5.1 only removed the frame_chain term) — none in KEPT → NEW
(collect_machine_roots) MUST structurally cover the whole machine or `missing` panics. Quiescence OLD = result ⊆ NEW
(light by design; the heavy non-vacuity is the midloop oracle). The `--with-oracle` MIN_BYTES=1 debug run on 744
fixtures is the empirical proof.

## VERIFY: `scripts/a5_greenwall.sh A55 --with-oracle` (slab 4325 / index 4177−walled-tests / conf 483·840 cycles /
0 oracle panics / lib 49 BOTH) + `scripts/a5_asan_both.sh A55` (slab full-conf 0 UAF — the #1 dropped-provider risk;
index M11-pt 0 UAF — and may NOW ADD stress_multidir-CLI+MIDLOOP since A5.4 fixed the gap) + 20-run determinism.

## RISKS: (1) the slab-pool-wrapper compile-blocker (§ catch — MUST wall). (2) cfg-inversion drops a slab root →
UAF (slab ASAN + slab oracle + 4325). (3) oracle vacuity (covered — midloop OLD keeps root_set). (4) test-mod compile
break in index (wall the registry tests; assert tests-only delta). (5) dead_code — walled (cfg'd, not unused) → clean;
verify clean `cargo check` not LSP. (6) scope: NO frame_chain (A5.6), NO deleting SAFEPOINT_ROOTS (A5.4 kept).
Commit msg: `A5.5 — registry core cfg→slab; index GC is registry-free (genuine CESK roots)`.
