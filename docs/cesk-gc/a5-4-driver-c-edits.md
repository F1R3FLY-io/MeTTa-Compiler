# A5.4 — fold driver-C onto the ONE narrow SAFEPOINT_ROOTS channel; retire the per-context seam

Verified by a Plan agent against the A5.0–A5.3 tree. Fixes the PRE-EXISTING CLI/REPL driver-C gap
(A5.1 ASAN: `stress_multidir`-via-CLI → `|KEPT|=0 |missing|=8005` — the index midloop oracle's
`ctx.collect_driver_roots` is a no-op when a nested bytecode VM is on the stack (`VmEvalContext`), so
the 8000 remaining `source` directives are unrooted → would-be UAF in release).

## CHOSEN APPROACH: publish-to-SAFEPOINT_ROOTS (NOT populate-gc_roots)
The CLI (`main.rs` batch) + REPL publish their control C (`source` ∪ `output` snapshot) to the GLOBAL
narrow channel via `register_temporary_roots`, HOLDING the `SafepointRootHandle` for the whole eval
loop. `collect_safepoint_roots` is ctx-INDEPENDENT (global registry) → the midloop covers driver-C
regardless of SessionContext vs VmEvalContext → resolves the VM-nested gap STRUCTURALLY. populate-gc_roots
is a dead end for index (A5.3 cfg-walled the `MettaStateGcRoots` provider out of index; the index
collector reads driver-C only via `collect_driver_program_roots` at quiescence + `collect_safepoint_roots`).

## CURRENT mechanism map (verified line numbers)
- payload: `MettaState::collect_driver_program_roots` metta_state.rs:239-248 (reads gc_roots.source/output) — **KEEP**.
- seam (REDUNDANT, retire): `EvalContext::collect_driver_roots` default no-op context.rs:83-101; SessionContext override session_context.rs:173-181. Production callers: ONLY eval_loop.rs:3665 (midloop oracle KEPT) + :3761 (midloop collection). VmEvalContext/Static/ParallelBranch/Jit all inherit the no-op (← the gap).
- quiescence flips read `state.collect_driver_program_roots` DIRECTLY: eval/mod.rs:294, tier_forced.rs:321; quiescence oracle KEPT roots.rs:404. (These are at TRUE quiescence where `state` is in scope ⇒ sound ⇒ KEEP as defense-in-depth.)
- SAFEPOINT_ROOTS transport (KEEP): gc_allocator.rs:3771-3844 (`register_temporary_roots`:3807, `collect_safepoint_roots`:3837, `SafepointRootHandle`+Drop:3781-3794 — Drop frees the slot ⇒ HOLD the handle). Existing pubs: conformance `all` mtt_conformance.rs:189; CLI/REPL filtered_results main.rs:775/1033; CACHE_ROOT_HANDLE eval/mod.rs:106/126.
- coverage today: conformance YES (never drains gc_roots.source; midloop off); CLI quiescence YES / **midloop NO**; REPL same as CLI.

## EXACT EDITS
1. **CLI batch** (main.rs, after the source snapshot ~641, before the `for expr in source_exprs` loop ~653):
   `let _driver_c_handle = { let mut dc: Vec<MettaValue> = Vec::new(); state.collect_driver_program_roots(&mut dc); register_temporary_roots(dc) };`
   Bind to a SCOPE-HELD name (drops at fn end ~802, after all evals). NOT `let _ =` (drops at `;` → re-introduces the UAF).
2. **REPL** (main.rs run_repl, after source snapshot ~1011-1012, before the `for` ~1014): same publication; `_driver_c_handle` drops at the match-arm end ~1054.
3. **Retire trait method**: DELETE `EvalContext::collect_driver_roots` default (context.rs:83-101).
4. **Retire override**: DELETE SessionContext::collect_driver_roots (session_context.rs:173-181). (`self.state` + `MettaState` import stay — used by `state()`.)
5. **Midloop oracle KEPT** (eval_loop.rs:3664-3669): drop the `ctx.collect_driver_roots(&mut driver_c_vals)` line; keep `collect_safepoint_roots(&mut driver_c_vals)` alone. Update the panic-message clause (d) to "...not published to SAFEPOINT_ROOTS via register_temporary_roots".
6. **Midloop collection** (eval_loop.rs:3760-3764): drop the `ctx.collect_driver_roots(&mut midloop_roots)` line at :3761 (collect_safepoint_roots at :3764 now carries driver-C).
7. **Comment corrections** (no logic): gc_allocator.rs:3834, roots.rs:382/398-402, eval_loop.rs:3623/3660/3666/3762, metta_state.rs:236-237, eval/mod.rs:298, tier_forced.rs:323 — "driver-C published to SAFEPOINT_ROOTS (global narrow channel); per-context seam retired; provider-registry coupling is A5.5".
NO structural change to collect_all_roots / RootProvider registry / models/mod.rs re-exports (= A5.5).

## VERIFY
- **LOAD-BEARING (the gap fix)**: `cargo build --features index-gc` (debug) + `METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_MIDLOOP=1 METTATRON_INDEX_GC_REPORT=1 ./target/debug/mettatron examples/cesk-gc/stress_multidir.metta` → **0 oracle panics** (was |KEPT|=0). THE A5.4 acceptance gate.
- Green-wall `scripts/a5_greenwall.sh A54 --with-oracle`: slab 4324 / index 4176 / conf 483 / 0 panics.
- ASAN: slab (seam removal touches both builds) + index — and **index ASAN can now ADD stress_multidir-CLI+MIDLOOP** (gap fixed) alongside M11-pt.

## RISKS
1. **Handle lifetime (#1)**: `SafepointRootHandle::Drop` frees the slot — bind `let _driver_c_handle = …;` over the WHOLE loop; NEVER `let _ =` or an early `drop()`. (Mis-binding re-introduces the exact UAF A5.4 fixes.)
2. Don't republish a SHRINKING source per-iteration (window of unrooted directive); one entry-time snapshot covers all remaining (sound, slightly over-approximate).
3. Byte-identical slab (seam removal invisible to slab — slab reads collect_all_roots; the 2 register_temporary_roots calls append+drop a slot, no-op in slab).
4. Oracle non-vacuity preserved (KEPT only gains the global driver-C term; NEW unchanged).
5. Stale comments invite a future agent to wrongly delete register_temporary_roots/collect_driver_program_roots — correct them (#7).
6. No scope creep into A5.5 (collect_all_roots/registry untouched).
