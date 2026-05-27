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

### Sequencing decision (data-informed; all stages will be completed end-to-end)

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
