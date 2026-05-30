# A5.6 — frame_chain.rs cfg→slab-only (the INDEX build has NO frame_chain module)

Verified by a Plan agent against the A5.0–A5.5 tree. Endgame: `eval/mod.rs:11` →
`#[cfg(not(feature = "index-gc"))] pub(crate) mod frame_chain;` so frame_chain isn't compiled in
index at all (physical file delete = F4). For that to compile, EVERY index-live `frame_chain::*` ref
must be cfg-walled first. A5.1/A5.2/A5.3 walled the spine/VM/ExprVec/provider sites; A5.6 walls the
remaining PARALLEL/WORKER paths + the mod decl. SLAB byte-identical (frame_chain compiled+used in slab).

## Index-live frame_chain refs to wall (W1–W8, the audit) — all in both-builds (non-cfg'd) parallel/worker fns
- W1 eval_loop.rs:184 `collect_frame_chain_roots` in `worker_cooperative_safepoint`
- W2 eval_loop.rs:2363-2364 `EvalFrameGuard::push_custom`+`FrameLabel::Custom("parallel-branch")` in `parallel_dispatch`
- W3 eval_loop.rs:2661 `collect_frame_chain_roots(&mut parent_roots)` in `pump_parallel_wait`
- W4 eval_loop.rs:2762 `collect_frame_chain_roots(&mut parent_roots)` in `pump_parallel_collapse_wait`
- W5 eval_loop.rs:2860-2861 `EvalFrameGuard::push_custom`+`FrameLabel::Custom("parallel-collapse")` in `parallel_collapse_dispatch`
- W6 types.rs:22 `use …frame_chain::EvalFrameGuard;` (top-level import)
- W7 types.rs:202 `pub _root_guard: Option<EvalFrameGuard>` in ParallelDispatchHandle
- W8 types.rs:344 `pub _root_guard: Option<EvalFrameGuard>` in ParallelCollapseDispatchHandle
(Already-walled by A5.1/2/3: vm/mod.rs slab arms, expr_vec_frame slab_gc, eval_loop spine-guard slab arm + oracle-OLD, vm/tests frame_chain tests. Confirmed.)

## Why walling W1–W8 drops NO live index root (single-threaded gate)
Index collector gate = `gc_mode_is_index() && !worker_ever_spawned() && active==0`. The parallel
frame_chain pushes (W2/W5) are created in the SAME fn that calls `note_worker_spawned()` (eval_loop
2509/2995) BEFORE spawning → the instant a parallel `_root_guard` exists, `worker_ever_spawned()`=true
(sticky) → gate CLOSED forever → the index collector never marks while a parallel frame_chain entry is
alive. W1/W3/W4 only run on worker closures (flag already set). So `collect_frame_chain_roots` feeds
nothing in index, and the collector reads `collect_machine_roots` structurally anyway (never frame_chain).

## EDITS (dependency order; apply before the mod wall)
- A types.rs:22 — `#[cfg(not(feature="index-gc"))]` on the EvalFrameGuard import.
- B/C types.rs:202/344 — `#[cfg(not(feature="index-gc"))]` on each `_root_guard` field. (KEEP `root_frame: Box<…>` + `_root_provider_arc` UNCONDITIONAL — constructed in both builds; #[allow(dead_code)] on root_frame already covers index.)
- D eval_loop.rs:2358-2368 — `#[cfg(not)]` on `let root_frame_ptr` (2358) + the `let root_guard = unsafe{…push_custom…}` block. KEEP `root_frame` Box (2354) unconditional.
- E eval_loop.rs:2563 — `#[cfg(not)]` on the `_root_guard: Some(root_guard),` struct-literal field (pairs with B).
- F eval_loop.rs:2857-2865 — like D for parallel-collapse.
- G eval_loop.rs:3032 — `#[cfg(not)]` on `_root_guard: Some(root_guard),` (pairs with C).
- H eval_loop.rs:184 — `#[cfg(not)]` on the `collect_frame_chain_roots(&mut roots)` line (roots stays mut — extend_from_slice@185 writes it in both arms).
- I eval_loop.rs:2661 — `#[cfg(not)]` on `collect_frame_chain_roots(&mut parent_roots)` (parent_roots stays live via the stable_branches/results loops).
- J eval_loop.rs:2762 — like I for collapse.
- K eval_loop.rs:2233/2244 — `#[cfg(not)]` on `collect_parallel_branch_frame_roots` + `collect_parallel_collapse_frame_roots` (now zero index callers after D/F → dead_code else). **APPLY-TIME GREP**: `rg -n 'collect_bound_value_roots|collect_parallel_result_roots' src/` — if their ONLY callers are these two walled collectors, wall them too; if they have other index callers, leave them.
- L eval/mod.rs:11 — `#[cfg(not(feature="index-gc"))] pub(crate) mod frame_chain;` (the endgame).

## Stack-trace: NO re-homing. format_stack_trace/capture_stack_trace have ZERO production consumers
(grep-confirmed; only frame_chain.rs + its tests). FrameLabel already in frame_label.rs (A5.0, both
builds). They ride along to slab with frame_chain; pre-existing 49-warning-baseline dead-in-slab-non-test.

## Index nextest delta = EXACTLY −8 (the 8 #[cfg(test)] tests in frame_chain.rs)
`rg -c '#\[test\]' src/backend/eval/frame_chain.rs` = 8 (mod tests @253 is plain `#[cfg(test)]`, runs in
BOTH builds today since mod.rs:11 is unconditional). The vm/tests frame_chain tests already have _index
siblings (A5.1) → don't move. So: index nextest 4175 → 4167 (−8); slab unchanged 4324. ASSERT the delta
is exactly the 8 `frame_chain::tests::*` names (diff `cargo nextest list --features index-gc` before/after).

## VERIFY: `scripts/a5_greenwall.sh A56 --with-oracle` (slab 4324 / index 4167=4175−8 / conf 483·840 / 0
oracle panics / lib 49 both) + `scripts/a5_asan_both.sh A56`. The PRIMARY signal: `cargo check --features
index-gc` MUST compile with mod frame_chain walled (a missed index-live ref = hard error → grep+wall it).

## RISKS: (1) a missed index-live frame_chain ref → mod-wall compile error (final gate: `rg 'frame_chain'
src/ | grep -v frame_chain.rs | grep -v '//'` shows 0 non-comment hits outside cfg(not) scopes). (2) a
dangling parallel-handle field (B↔E, C↔G paired). (3) EDIT-K chain under/over-wall (the apply-time grep).
(4) index delta ≠ −8 (a hidden extra wall — diff the nextest list). (5) slab UAF from an inverted cfg
(slab arm byte-identical → low; slab ASAN + slab oracle guard). (6) unused_mut on roots/parent_roots (H/I/J
— mitigated: writers in both arms). Commit: `A5.6 — frame_chain.rs cfg→slab-only (index has no frame_chain module)`.
