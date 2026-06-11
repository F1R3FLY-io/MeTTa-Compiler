# Allocator-progress (#273) + Audit-hardening — ordered plan

> Read-only assessment (Plan agent, 2026-06-10), branch feature/petta-semantics @ 4364e24a.
> Ordering: lowest-risk byte-identical formal/doc increments first; the large refactor (Finding 2) last.

## Already DONE (do NOT redo)
- **#272** (concurrent reuse pressure): `c89a53ce`; `formal/rocq/gc/ConcurrentReusePressureProgress.v` (admit-free) + `MC_ConcurrentBumpFreshOnly.tla` cfgs + pins (`verify_cesk_gc_source_coupling.sh:314-320, 986-994`). FORMAL COMPLETE.
- **#274** (quiescent side-index reuse): `266d19d9`; `formal/rocq/gc/QuiescentSideIndexReuse.v` (admit-free; +GenerationGuardSafety) + `MC_SideReclaimGeneration.tla` (_guard pass/_no_guard fail) + pins (`:317-320, 407-419`). FORMAL COMPLETE.
- Foundations: `SideFreeQuiescence.v`, `SideReclaimRefinement.v`, `MC_SideReclaimSnapshot.tla` (6 cfgs), `MC_SideFreeQuiescence.tla` (4 cfgs).
- The E1 V4 ASAN gate passes (GATE_RC=0) — the RUNTIME is bounded; remaining is the FORMAL proof for #273 only.

## INCREMENT 1 — Finding 3 comments + gen-sufficiency note (smallest; byte-identical; NO ASAN)
Pure comment edits + source-coupling pins; comments are stripped → byte-identical by construction.
- **`index_heap.rs:2796-2799`** STALE "unbounded by design (...grows to address the entire u32 index space...)" → CORRECT to: bump-allocated but RECYCLING (freed cells reused before the bump grows, post-266d19d), bounded by live-side high-water + reclaims-pending-drain, NOT append-only.
- **`index_node.rs:46-54`** (ChildRef `gen:u32` doc) APPEND the u32-sufficiency argument: a SideReclaim snapshot is captured at the cell's current gen and drained at the NEXT true-quiescence (`pending_side_major` forces it) within a bounded number of reuses of that one cell; aliasing needs 2^32 reuses BETWEEN capture and drain → unreachable. u64 would grow each `{idx:u32,gen}` Ref 8→16B (Node≤32B assert `index_node.rs:151-153`) for zero benefit. (The argument already lives in `QuiescentSideIndexReuse.v:201-205` + `SideReclaimGeneration.tla:10-11`; only the SOURCE comment is missing.)
- **(optional) `index_arena.rs:404-407`** add "(node slots; side columns recycle via per-cell generations — see index_heap.rs SideColumn)" to forestall node-vs-side conflation (the "never grown" claim is CORRECT for the node arena).
- **Pins**: 1-2 `line_no`/`assert_after_before` in `verify_cesk_gc_source_coupling.sh` near L407-419 asserting the corrected phrases (so they can't silently regress).
- **Verify**: proof-hygiene + source-coupling (new pins) + `cargo check --features index-gc` (49 warnings) + `cargo check` (slab). Skip the heavy greenwall (comment-only ⇒ byte-identical).

## INCREMENT 2 — CLOSE #273: bounded side-reclaim progress proof (byte-identical; design B ALREADY implemented)
Design B is implemented: `pending_side_major` (`index_heap.rs:2512`: `phase=="quiescence" && pending_side_reclaims>0`) → `do_major` (`:2527`); `free_pending_side_reclaims` (`:1332-1338`) drains the ENTIRE vec via `std::mem::take`; `append_pending_side_reclaims` (`:1247-1254`) appends one SideReclaim per reclaimed owner. NO `src/*.rs` logic change → byte-identical. This increment PROVES + PINS it.
- **New `formal/rocq/gc/RendezvousSideReclaimProgress.v`** (mirror ConcurrentReusePressureProgress.v style; admit+axiom-free). Theorems: `rendezvous_appends_one_per_node_reclaim`; `quiescence_drain_empties_pending`; `pending_side_trigger_forces_drain` (Quiescent ∧ pending>0 → DoMajor ∧ Drains); **MAIN `side_committed_bounded`** (committed_side ≤ live_side_high_water + reclaims_since_last_quiescence; each quiescence empties pending ⇒ no unbounded growth); non-vacuity `without_pending_trigger_side_grows_unbounded` (pre-266d19d, no trigger ⇒ accumulates).
- **New `tla/RendezvousSideReclaimProgress.tla`** + `MC_*.tla` wrapper + 2 cfgs: `_bounded` (`PendingSideTrigger=TRUE`) PASS on `PendingBounded`; `_unbounded` (FALSE) FAIL "Invariant PendingBounded is violated".
- **Pins** (extend `verify_cesk_gc_source_coupling.sh:1008-1035`): `pending_side_major` (`:2512`), in `do_major` (`:2527`), `mem::take` drain (`:1333`), + run_rocq/run_tlc presence asserts.
- **Harness**: `verify_cesk_gc_formal.sh:161` run_rocq after QuiescentSideIndexReuse.v; `:360-363` run_tlc after side_reclaim_generation (pos/neg). **Ledger** `docs/cesk-gc/formal-verification-ledger.md` (#273 entry + the design-A-deferred-to-Finding-2 rationale `index_heap.rs:2365-2371`).
- **Verify**: proof-hygiene (new .v MUST be harness-wired) + tlc-hygiene + source-coupling + full formal harness (pos/neg discriminators hit exact strings) + greenwall both modes byte-identical. NO ASAN.
- pgmcp #273 → ~90% + progress-log.

## INCREMENT 3 — Finding 2: confine 'static (LARGE ~1400 sites; do LAST; needs ASAN + greenwall --with-oracle)
Root: `metta_value_trait.rs:168` `as_atom→Option<&'static str>`; the 'static points into the thread-local INNER_SHADOW (`metta_value.rs:881-918`, valid only until `clear_inner_shadow`). Tie to &self: trait (`:168/180/283`) + inherent (`metta_value.rs:1585/1657/1676/1160`) + trait-impl (`:2897`) + JIT `bytecode/jit/types/value.rs` + the HARDER `MettaValueInner` fields (`:656/666/720` `Atom(&'static str)` etc — need a backing-store borrow). 682 as_atom + 456 as_sexpr + 265 view; 309 let-bound as_atom = the audit set. Strategy: change trait+impls, let `cargo check` enumerate breakages, fix the STORING sites (clone-to-owned or narrow scope); most use-then-drop recompile unchanged. Borrow checker = the proof. Extend `TrackedVarSideRetention.v` / add `LaunderedRefConfinement.v` + flip the Finding-1 'static pins (`verify_cesk_gc_source_coupling.sh:243-257`). Verify: full ladder + greenwall --with-oracle (483/0/0, warnings 49/49) + `e1_flip_v4_asan.sh` GATE_RC=0. FOLLOW-ON: design A (#273) — drain side every rendezvous, now safe since no laundered side ref escapes &self.

## Then: E5 loom/TSan (#256/#257), Phase F (F1 Welch bench / F2 --gc reporter / F3 index-default / F4 delete slab), final composition (#152 done) + whole-system harness (#22), pgmcp reconcile (#21).

## INCREMENT 3 — RESOLUTION (2026-06-11): largely SUPERSEDED; residual closed as doc + pins
Source-verified against HEAD (post atom-interning `c7744140` + Finding-1 `895378bf`):
- The ROOT (`as_atom -> Option<&'static str>` laundering INNER_SHADOW bytes) was FIXED by
  permanent atom interning: `Node::Atom(&'static str)` holds `symbol::intern_static` bytes
  owned by the perpetual interner — honest `'static`, never freed by any sweep (PROVEN:
  `formal/rocq/gc/InternedAtomNeverFreed.v`; the trait doc now states this and is pinned).
- The slice/str accessors (`as_sexpr`, `as_string`, `as_error`, …) are ALREADY `&self`-tied
  in the trait — the confinement this increment prescribed exists; no 1400-site refactor is
  needed. (A `&self` tie on a `Copy` handle cannot, by itself, forbid holding across a
  safepoint — that hazard class was closed by the Finding-1 fix + `TrackedVarSideRetention.v`
  + the Finding-1 source-coupling pins, which audit/guard the holders.)
- Side-payload reuse aliasing is closed by the per-cell generations (`QuiescentSideIndexReuse.v`)
  and bounded drain (`RendezvousSideReclaimProgress.v/.tla`).
Residual delivered: the corrected `as_atom` trait doc (+ pin), the Finding-3 Increment-1
comment corrections (+4 pins), and this resolution note. The FOLLOW-ON idea (drain side every
rendezvous — design A for #273) remains explicitly NOT taken: design B (pending_side_major
quiescence drain) is the proven, shipped mechanism.
