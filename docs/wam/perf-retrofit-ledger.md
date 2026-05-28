# WAM-on-T0 Retrofit + MM2 Query Acceleration — Scientific Ledger

Companion to `control-substrate-design.md` and `trail-binding-model.md`. Tracks the
benchmark-gated optimization initiative per the plan at
`~/.claude/plans/my-previous-session-listed-frolicking-octopus.md`.

Hardware: AMD, 32 cores, `performance` governor. **CCD0 = cores 0–7** (share L3).
Benchmark protocol: `taskset -c 0-7` (CCD0 pin); release binary for wall-clock; the
`[profile.profiling]` binary (LTO + `debug=1`, unstripped) for `perf`. PLN runs may be
capped with `systemd-run --user --scope -p MemoryMax=8G -p CPUQuota=400%`.

---

## Stage 0 — Baseline + determinacy census + profile (2026-05-27, HEAD `aa3fe3e`)

### Wall-clock baselines (CCD0-pinned, release `target/release/mettatron`)

| Workload | Mean | Notes |
|---|---|---|
| PLN Smokes | 155.7 ms ± 4.8 ms | hyperfine -r 10 |
| PLN Toothbrush | 1.712 s ± 0.023 s | hyperfine -r 5 |
| mmverify-demo0 | ~2.0 s (1.92–2.10) | 3× /usr/bin/time |
| PLN Robot | ~6.0 s (5.69–7.29), ~1.9 GB RSS | 3× /usr/bin/time; high variance |
| PLN FlyingRaven | ~20.9 s (20.69–21.16), ~1.9 GB RSS | 3× /usr/bin/time |

FlyingRaven is the headline perf target. All future per-stage deltas measured against
these, freshly re-captured each stage (CCD0-pinned, CPU quiet).

### Determinacy fraction (uncertainty #1 — RESOLVED)

**93.3% of rule dispatches produce exactly one match** — documented at
`eval_loop.rs:955` and consistent with the already-present single-match fast path
(`eval_loop.rs:958`). The fan-out skip (planned Stage 3b) therefore **already exists**
for the common case; the remaining determinacy work is narrower than planned (see below).

### FlyingRaven self-time profile (`perf record --call-graph dwarf -F 499`, profiling binary)

DSO split: **62.9% mettatron, 30.5% libc.so.6.**

| Symbol | Self-time | Interpretation |
|---|---|---|
| libc `0x172xxx` cluster (memcpy/memmove family) | **~20%** | structure-copying: freshen + apply_bindings arena rebuilds |
| `core::slice::sort::…quicksort<usize>` | 5.64% | `incremental_gc.rs:437 scratch_marks.sort_unstable()` (GC mark set) |
| `collect_variables_generic` | 5.12% | called by freshening + apply_bindings |
| `collect_subgoal_roots` (tabling) | 4.53% | tabling root collection |
| `expr_contains_cut` | 4.08% | per-dispatch `any_match_cuts` scan at `eval_loop.rs:941` |
| `MettaValue::as_atom` | 3.94% | tree-walk leaf |
| `MettaValue::as_sexpr` | 3.20% | tree-walk leaf |
| Sip13 hash `write` | 2.22% | HashMap / seen-set hashing |

### Conclusions (which clone is the memcpy — uncertainty #2 RESOLVED)

1. **The ~20% memcpy is structure-COPYING, not env-fork.** The single-match fast path
   already skips the fork for 93.3% of dispatches, so `fork_for_nondeterminism` is
   **not** in FlyingRaven's hot path. ⟹ **Stage 3a (freshening alloc) + Stage 3c
   (structure-sharing apply_bindings) are the biggest levers; Stage 3d (lazy fork) is
   lower-value than planned** (revisit only if a fork-heavy workload shows it).
2. **NEW cost center (~10%) not weighted in the plan: GC mark-sort
   (`scratch_marks.sort_unstable`, 5.64%) + tabling root-collection
   (`collect_subgoal_roots`, 4.53%).** Candidate for a dedicated stage (Stage 3e).
3. **`expr_contains_cut` (4.08%) is a cheap, low-risk win:** at `eval_loop.rs:941` the
   `any_match_cuts` scan re-walks every instantiated RHS per dispatch. Substitution
   cannot introduce a `(cut)` that the rule body lacked, so the precomputed
   `RuleEntry::body_contains_cut` flag already answers this for the determinate path.
4. **`match_space`/`query_multi` is ABSENT from FlyingRaven's hot path.** ⟹ the MM2
   ProductZipper work (Stage 1) accelerates **conjunction-heavy / large-KB query
   workloads** (PLN `ruletests/transitiveSimilarity.metta`, `memberDeductionA.metta`,
   synthetic joins) — it will **not** move the FlyingRaven headline number. Stage 1
   remains valuable for those workloads and the explicit "exploit MM2" goal, but must be
   gated on conjunction benchmarks, not FlyingRaven.

### Plan's own re-prioritization gate

The plan's Stage 0 gate ("if determinacy <30% OR freshen+apply <10% self-time,
re-prioritize") is **not tripped**: determinacy is 93.3% and freshen+apply (memcpy ~20%
+ collect_variables 5.12%) far exceeds 10%. Stage 3a/3c are confirmed high-value.

### Status: Stage 0 COMPLETE.

### Stage 1b — single-pattern `match_space` trie-acceleration: CONFIRMED WIN (✓ landed)
- **Finding (match-heavy benchmark, 5000 ground facts × 3000 `(match …)` queries):** the
  production `match_space` (core.rs) does a linear O(|btm|) scan per query — **40.6–41.0 s,
  15.6 GB RSS**. (match_space is absent from FlyingRaven's profile, but dominates
  match-heavy/large-KB workloads.)
- **Change:** `match_space_btm_query_multi` — wraps the pattern as `(, pattern)` and uses
  MORK `query_multi`/`ProductZipper` (O(matches)) for the `btm` store; `match_space` gates it
  on a ground space (`variable_fact_count == 0` + `variable_atoms` empty) and a non-`=` head
  (rules are De-Bruijn variable atoms in btm — fall back to the linear+bidirectional scan for
  them and for non-ground spaces). Wide-btm scan unchanged.
- **Result:** **40.6 s → 26 s (~36% faster), 15.6 GB → 9.8 GB RSS (~37% less)**; correctness
  `[b2500]` ✓; FlyingRaven unchanged (18.08 s, ✅✅). Residual ~26s is `query_multi`'s
  per-call 4 GiB buffer reserve + per-query `create_space` CoW clone (a MORK-internal cost
  shared by both paths; further amortization is a separate MORK-side concern).
- **Correctness fix:** initial version mishandled `=`-headed rule queries (rules are
  De-Bruijn, not counted by `variable_fact_count`) → `test_rules_with_multiplicity` failed;
  added the `=`-head exclusion. Gate: nextest re-run (4239) green.

## Stage 5b — MM2 §10 reduction sinks in exec: COMPLETE (✓ landed)
- `try_eval_reduction_sink_generic` (mork_forms.rs): for an exec consequent that is a
  single reduction-sink O-template `(O (<sink> ctx slot e))`, aggregate `e` over ALL
  antecedent matches Θ and insert `ctx[slot ↦ Sym(result)]` ONCE (vs the per-match
  O-dispatch). Heads: `count`→|Θ| (SNK-COUNT §10.7), `sum`→Σ decimal-u64 (SNK-SUM §10.9),
  `fsum`/`fmin`/`fmax`/`fprod`→f64 reduction (SNK-FRED §10.10), `and`→boolean-AND,
  `hash`→order-insensitive FNV-1a digest. Unparseable numeric input → graceful
  fall-through (not a Tier-1 panic).
- Wired into `eval_exec_generic` (the now-firing exec from 1c is the consumer). Validated:
  exec `(O (count (result $n) $n $x))` over 3 facts → `(result 3)`; `(O (sum …))` →
  `(total 42)`; `(O (fsum …))` → `(ft 3.75)`. **Conformance 483/483** (new fixtures
  M14/005-exec-count-sink, M14/006-exec-sum-sink); nextest 4239; PLN 5/5.

## Stage 6 — rule-index audit: justified NOT built (data-driven)
- The index chain `by_head_arity` (HashMap) → `by_first_arg_head` (HashMap) →
  `DiscriminationTree` → empty-first-arg prune → `PerHeadAtomIndex` bloom is already
  **deeper than classical WAM first-argument indexing** (which only keys on arg-1's
  principal functor).
- FlyingRaven profile confirms it is NOT a bottleneck: `get_candidates_filtered` **0.04%**,
  `HeadArityBloomFilter::may_contain` 0.03%, `AtomicBloomFilter::may_contain` 0.15%,
  `DiscNode::clone` ~0%. (A separate 0.6%/0.4% `get_all_rules` FlatMap is the no-head
  wildcard-fallback iteration, not the indexed path, and is below the optimization
  threshold.)
- Deepening the index (second-arg indexing, deeper disc-tree) would optimize a
  sub-0.5% non-bottleneck — a violation of the data-driven mandate. **Justified not built**;
  the existing index is WAM-grade+ and the determinacy data (93.3% single-match) shows it
  already cuts candidate sets to ~1 before unification.

## Sequencing decision (data-informed; all stages will be completed end-to-end)

Per the plan's data-gating principle, Stage 0 findings refine — but do not drop — any
stage. Full execution order:

1. **Stage 1** (MM2 ProductZipper conjunctive join) — approved-first; independent of the
   engine internals (low interference); delivers the explicit "exploit MM2" capability.
   Gated on **conjunction-heavy benchmarks** (PLN ruletests + synthetic large-KB joins),
   NOT FlyingRaven (which the profile shows is query-independent).
2. **Stage 2** (MM2 streaming + in-callback aggregation).
3. **Stage 3a** (freshening alloc) — attacks the ~20% memcpy. Biggest FlyingRaven lever.
4. **Stage 3c** (structure-sharing apply_bindings) — compounds with 3a on the memcpy.
5. **Stage 3e (NEW, data-driven)** — GC mark-sort (`incremental_gc.rs:437/728
   sort_unstable`) + tabling root-collection (`collect_subgoal_roots`), ~10% combined on
   FlyingRaven. Not in the original plan; added because the profile measured it.
6. **Stage 3b** (determinacy fast-path) — narrowed: the 93.3% single-match fast path
   already exists, so this targets the *residual* (the `expr_contains_cut` per-dispatch
   re-scan at `eval_loop.rs:941`, 4.08%). NOTE the correctness subtlety: substitution
   *can* introduce an unquoted `(cut)` via a binding value (e.g. rule `(= (run $k) $k)`
   called `(run (cut))`), so the precomputed `RuleEntry::body_contains_cut` is a fast
   negative only when no binding value carries a cut — handle precisely, don't naively
   replace the scan.
7. **Stage 3d** (lazy/COW fork) — lower-value per data (fork not in FlyingRaven hot path),
   but completed; benchmark on a fork-heavy workload to find where it pays.
8. **Stage 4** (T0 trail) — micro-benchmark in isolation; integrate if it wins, else
   document-as-refuted-and-revert (a complete result).
9. **Stage 5** (MORK ACT + reduction sinks) — authorized MORK edits.
10. **Stage 6** (indexing audit) — measure with the candidate-size histogram; conclude.

Each stage: fresh CCD0 baseline → implement → `hyperfine` + `perf` confirm/refute →
correctness gate (nextest ~4235, mtt-conformance --strict 481, M11-pt 221, M11-he 40,
PLN 5/5) → commit-or-revert with documentation here.

---

## Stage 1 — MM2 ProductZipper conjunctive join (2026-05-27): INFRA COMPLETE + VALIDATED; production wiring deferred-by-design to after engine stages

### What was built (all additive, validated, gate-green)
- `mork_convert.rs::mork_bindings_to_generic<V,F,M>` — value-generic ns-0 binding
  converter (mirrors how `mork_expr_to_metta_value` delegates to
  `mork_expr_to_generic_value`); `mork_bindings_to_metta` kept as the `SmartBindings`
  specialization for the existing single-pattern path.
- `atom_space.rs::variable_fact_count: AtomicUsize` — monotonic conservative
  completeness gate, maintained at all 5 `AtomSpace` literals (new/fork/make_owned/
  union/merge_all_modified) + incremented in `add_to_space`/`add_to_space_shared`
  (rules excluded via their early-return).
- `core.rs::match_conjunction_query_multi` — wraps goals in `factory.conjunction(…)`,
  runs MORK `query_multi`/`ProductZipper`, reconstructs bindings via
  `mork_bindings_to_generic`. Completeness gate: non-empty & <63 goals, no `=`-headed
  goal, `variable_fact_count==0`, `variable_atoms` empty → else `None` (caller falls back).
- 2 validation tests (`core::tests::test_match_conjunction_query_multi_*`) — **PASS**.
  Empirically confirm the binding reconciliation: `(, (parent $x $y)(parent $y $z))` over
  ground facts → exactly `{$x=a,$y=b,$z=c}` (pattern vars at MORK ns 0, shared De-Bruijn
  index), and the gate falls back on `=`-goals / empty / variable-fact-present.

### Key finding (reshapes Stage 1): no current production multi-pattern space-join consumer
- The planned wiring site `match_conjunction_goals_generic` (mork_forms.rs) backs ONLY
  the `(exec …)` antecedent, whose current mettatron behavior is a **v1.0 no-op** (the
  iterative `thread_bindings_through_goals_generic` returns empty for a matching
  antecedent, so the directive does not fire — verified against the preserved HEAD
  baseline binary). Wiring the functional ProductZipper join there made `(exec)` FIRE,
  breaking `t1_mork_forms::t1_exec_form_routes_to_t0_via_compile_time_gate`. A perf
  optimization must not change observable semantics → **wiring REVERTED**.
- This contradicts mm2-spec §6 **R-TPL-CC [N] (Normative)** which inserts templates
  (exec fires). The test's "§23.3" justification is a **phantom** (no such section in
  mettatron-specification/ or docs/). General conjunction eval
  (`eval_conjunction_step_generic`, step.rs:178) is **goal-solving** (evaluate + thread),
  not a space pattern-join, so it is not a ProductZipper consumer either.

### User decision (2026-05-27)
Pursue ALL of: (1b) accelerate the heavily-used single-pattern `match_space`
(core.rs:3176, currently a full linear btm scan) with the trie-pruned `query_multi`;
(1c) make `(exec)` MM2-conformant per R-TPL-CC and wire the ProductZipper into the
firing antecedent (updating the stale test); full ProductZipper integration. **Sequenced
AFTER the engine stages** (3a/3c/3e — measured FlyingRaven beneficiaries), per the user's
"engine stages next" choice. The validated infra stays in place (called by its tests),
changing no current semantics, until its turn.

---

## Stage 3a / 3c — REFUTED by profile (data-driven; not built)
- **3a (freshening alloc):** `freshen_with_epoch` 0.25%, `format!` 0.05%, `intern_fresh_name`
  0.04% — already optimized via `CachingRename`/`intern_fresh_name`. The Stage-0 "~20%
  memcpy = freshen rebuilds" attribution was WRONG.
- **3c (structure-sharing apply):** `apply_bindings*` <1% total, `sexpr_from_slice` 0.08%;
  the ~20% libc memcpy resolves as a leaf with NO `apply_bindings` caller. Sharing saves <1%.
- The ~20% libc memcpy is driven by GC root-collection Vec growth + slab alloc, not
  freshen/apply. ⟹ pivot to Stage 3e (GC).

## Stage 3e — GC nursery-threshold tuning: CONFIRMED WIN (✓ landed)
- **Change:** `NurseryConfig::default().threshold_bytes` 64 KiB → **512 KiB** (8×),
  `incremental_gc.rs`. The mark-set merge-vs-quicksort was already refuted (Phase 7); the
  lever is collection FREQUENCY (the ~13.5% GC/root cluster = freq × full-root-rewalk).
- **Result (CCD0, release, 3×):** FlyingRaven **20.94 s → 19.69 s (~6% faster)**; RSS flat
  (~1.90 GB, marginally lower); Robot 5.70 s → 5.50 s, RSS lower. Correctness: FlyingRaven
  both `✅`; PLN Smokes/Toothbrush/Direct(3/3)/DeductionRevision pass; **nextest 4239/4239**
  (incl. memory-constancy + `test_config_default` updated). Higher thresholds give
  diminishing returns (residual GC ≈1%), so 512 KiB is the trade-off (RSS-bounded).

## Stage 3b — `expr_contains_cut` per-dispatch scan: CONFIRMED (✓ landed; benchmark pending)
- **Finding:** the determinacy fan-out fast path already exists (93.3%, eval_loop.rs:958);
  the residual Stage-3b cost is the per-dispatch `any_match_cuts` walk over every
  instantiated RHS (`eval_loop.rs:941`, **4.08%** of FlyingRaven).
- **Change:** new monotonic `RuleIndex::any_rule_has_cut` flag (set at the single
  `add_rule` choke point from the precomputed `RuleEntry::body_contains_cut`; propagated by
  `#[derive(Clone)]` + union/merge re-adds). When no rule body uses cut (the common case —
  ALL of PLN: no `(cut)` operator), the dispatcher scans only the small binding values
  (cut-as-data) instead of the full instantiated RHS. Correct in both branches.
- **Gate:** nextest **4239/4239** incl. cut/once suites.

## Engine track combined result (Stage 3e + 3b/expr_contains_cut)
- **FlyingRaven (CCD0, release, 4×):** baseline `aa3fe3e` 20.70–20.84 s → combined
  **17.99–19.27 s (~11–13% faster)**. mmverify-demo0 flat (1.90 s — doesn't exercise the
  GC/cut paths). Correctness: FlyingRaven `✅✅`; **nextest 4239/4239**.
- Net: 3e (GC threshold, ~6%) + 3b (expr_contains_cut, ~6%) compose to a ~12% headline win,
  both data-confirmed and gate-green. 3a/3c/3d/4 refuted-and-documented (the engine was
  already well-optimized on those axes). **Engine track complete.**

## Stage 3d / Stage 4 — REFUTED by profile (data-driven; not built)
- **3d (lazy/COW fork):** `fork_for_nondeterminism` does not appear in the FlyingRaven
  profile (`make_owned` 0.01%) — env-fork is NOT a bottleneck (the single-match fast path
  already skips the fork 93.3% of the time, and Gap A already Arc-shares atom_space/rule_index).
- **Stage 4 (T0 deterministic-path trail):** its premise was residual
  `compose_outer_inner`/`project_carrying` cost — but those are 0.12% + 0.06% (~0.2% total)
  in the profile. A trail would save ~0.2% and re-incur the `unification.rs` reverted-WAM
  per-call churn. Not built; the `binding_store.rs` trail stays inert/ready (consistent with
  the `control-substrate-design.md` "barrier-identity, not a choice-point stack" decision).

## Stage 5a — ACT out-of-core persistence: COMPLETE (true MM2 join + full MeTTa surface)
- **User decision:** "Build the true MM2 join first, then wire the full MeTTa surface."
- **Discovery (interning):** the planned `query_multi_i` `(I (ACT name pat))` source form
  byte-matches **inline** symbol markers in `ASource::new`; MeTTaTron's `interning` build
  encodes them as interned IDs → `unreachable!()` (`sources.rs:313`, verified empirically).
- **Part A — true join (authorized MORK add):** new `Space::<()>::query_multi_act`
  (`MORK/kernel/src/space.rs`) — the ACT analogue of `query_multi`: a `ProductZipperG` over
  the mmap'd ACT's read-zippers, taking the **interned** conjunct pattern directly (no inline
  markers), trie-pruned (O(matches)). Bindings at namespace 0.
- **MeTTaTron (`src/backend/environment/act_persistence.rs`):** `save_space_to_act`
  (`dump_from_zipper`, multiplicity→u64 leaf), `query_act` (trie-pruned join + mmap-scan
  fallback), `load_space_from_act` (`read_zipper_u64` multiplicity-faithful restore). No
  MORK/PathMap value-genericization needed.
- **Part B — full MeTTa surface (`step/sexpr.rs`):** `(save-space! "n")` → path String;
  `(load-space! "n")` → Σ-multiplicity Long; `(query-act "n" pat tmpl)` → superposed matches.
  Classified impure (`dispatch_hints.rs::is_impure_head`, never memoized — `file-*!` precedent)
  and T0-only (`bytecode/mod.rs::can_compile_with_env => false`, `exec` precedent).
- **Out-of-core:** the KB lives in the file-backed mmap; one fact materialized at a time —
  never the whole KB in heap.
- **Completeness pass (deferrals closed, no-deferrals mandate):**
  - **Bag-faithful query** — `query_act` emits one copy per unit of stored multiplicity (==
    `match_space` multiset): scan reads the `u64` leaf; the join recovers it via an O(depth)
    `u64`-zipper descend (`act_leaf_multiplicity`).
  - **Wide expressions** — `save`/`load`/`query` cover `wide_btm` (arity ≥ 64) via the
    `<name>.wide.act` sibling (self-describing Wide MORK, sm-independent → cross-run for free).
  - **Cross-run persistence** — `save` serializes the `SharedMapping` to `<name>.sm`
    (`mork_interning::SharedMapping::serialize`); `query`/`load` decode via it (`act_sm_for`),
    so a *fresh process/env* decodes a snapshot. Join (env-sm encode) is the intra-run fast
    path; on a cross-run miss `query_act` falls back to the sm-faithful scan.
- **Tests (14 new):** 8 Rust-API (`act_persistence::tests` — incl. bag, wide, cross-env) + 6
  surface integration (`tests/act_surface.rs`). **Gate: nextest 4252/4252, mtt-conformance
  --strict 483/483, PLN-main 7/7 (0 ❌).**
- **Next layer:** see Stage 5a-LSM below (now DELIVERED).

## Stage 5a-LSM — LSM tiered ACT-backed mutable space: COMPLETE (2026-05-27)
- **Scope:** turn an out-of-core ACT into the *primary* store of a live, mutable space — the
  "next layer" the base Stage 5a deferred. `match_space` = `overlay ++ (base − tombstones)`.
- **New module `src/backend/environment/act_tiered.rs`** + 3 fields on `AtomSpace<V>`
  (`act_base: RwLock<Option<Arc<ActBase>>>`, `tombstones: RwLock<PathMap<Multiplicity>>`,
  `has_act_base: AtomicBool`), threaded through `new`/`fork`/`make_owned`/`union`/
  `merge_all_modified` (drop base on genuine merge; Arc-share on branch-union/fork paths;
  `fork_for_nondeterminism` shares the whole `atom_space` for free).
- **Read (4 entry points gated):** `match_space`/`match_space_exists`/`match_space_first`/
  `match_space_query_multi` gate on `has_act_base` — a relaxed-acquire load on the bloom-miss /
  end-of-overlay branch only, so the **no-base hot path is byte-identical** (Hard Constraint
  met; verified by `no_base_fast_path_unchanged` + the unchanged full gate). Base half via
  `match_space_base` — reuses `query_act`'s trie-pruned `query_multi_act` join (head-shaped) +
  leaf-scan fallback, tombstone-filtered (`effective = max(0, base_mult − tombstone(key))`),
  bag-faithful, overlay-first, **fully generic** (`mork_bindings_to_generic`). The overlay bloom
  early-return is bypassed when tiered (bloom tracks overlay heads only).
- **add/remove (exact bag inverses):** add un-tombstones-OR-overlay-writes (XOR, not both);
  remove peels overlay first, else tombstones a base copy (capped at base_mult). Interposed in
  `add_to_space[_shared]` / `remove_from_space[_shared]`, gated on `has_act_base`, literal-fact
  scoped.
- **Compaction `(compact-space! "n")`:** decode `base − tombstones` → re-add to overlay → dump to
  TEMP → atomic `rename` over `<n>.{act,wide.act,sm}` → invalidate sm-cache → clear overlay →
  RCU re-attach. Returns Σ-multiplicity Long. Semantic no-op on the visible multiset.
- **Surface (T0, impure):** `(attach-act-base! "n")`→path, `(detach-act-base!)`→Unit,
  `(compact-space! "n")`→Σ Long. Added to `is_impure_head` + `can_compile_with_env => false`.
- **Deviations from the design (all justified, documented in `act_persistence.rs` §4):**
  (1) `ActBase` stores the base NAME + sm, NOT the `ACTMmap` — `ACTMmap` has an interior
  `Cell<u64>` → `Send` but `!Sync`, and `AtomSpace` must be `Send + Sync` for `Arc`-shared cross-
  thread eval; the read re-opens the mmap per query (the proven `query_act` pattern).
  (2) Tombstones are a COUNT of base copies to suppress (separate `PathMap`), NOT 0-valued `btm`
  entries — the overlay's `remove_atom` auto-prunes 0-count entries.
  (3) add does un-tombstone XOR overlay-write (not "both" as the prose read) — doing both
  double-counts; the XOR model is the unique bag-exact `add∘remove = id` semantics.
  (4) No `join_eligible` distinction — the `mork_interning` deserialize fix makes cross-run
  trie-pruned joins faithful, so the base always uses its own `<name>.sm`.
- **Tests (28 new):** 19 Rust-API (`act_tiered::tests`) + 9 surface (`tests/act_tiered_surface.rs`).
- **Gate (after EACH of phases 0–3): nextest 4282/4282, mtt-conformance --strict 483/483,
  PLN-main 7/7 (0 ❌).** Full design: `docs/mm2-integration/act-out-of-core.md` §4.
