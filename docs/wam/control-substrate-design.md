# MeTTaTron Logic-Programming Control Substrate — Design & Ledger

**Status:** design complete 2026-05-26; implementation in progress (7 phases, benchmark-gated).
**Supersedes the framing of** `trail-binding-model.md` (whose motivating binding-drop bug was
already fixed by commits 663c7e1/880c415/5637128 — the trail is re-scoped here to a mark/undo
choice-point backbone, NOT a binding cache).
**Bar:** best-performing, best-architected, most-expressive — NOT mere functional equivalence.

## Central architectural decision: Cut-Barrier Identity, not a WAM choice-point stack

MeTTaTron's T0 trampoline is a CESK/SECK machine. Nondeterminism is **not** a single WAM
choice-point stack — it is a heterogeneous heap continuation forest across FIVE fan-out
continuation variants (`src/backend/eval/trampoline/types.rs:467`):
- `ProcessRuleMatches` — rule-dispatch alternatives (the ONLY site cut currently targets)
- `ProcessAmb` — `superpose`, the `let*` multi-result fallback, foldl fan-out
- `ProcessMatchSpace` / `ProcessMatchTemplates` — `match` over a space → N templates
- `ProcessConjunction` — DEAD for source-eval (only reachable from MORK-deserialized `Conjunction`)
- `ProcessLetStar` — threads a multi-result value-expr by rebuilding nested `let` → `ProcessAmb`

Per-branch bindings ride in each `BoundValue`'s `SharedBindings` sidecar, composed at every
fan-out (`compose_outer_inner_strict_generic`) and projected (`project_carrying_for_consumer`).

A monolithic `K-Cut-Barrier` that pops a choice-point stack (as `06-pt-semantic-mapping.md:117`
imagines) does NOT map — there is no single stack. The optimal, **more expressive** design is a
**uniform cut-barrier identity** threaded through every fan-out continuation, consumed by one
shared predicate. It is scope-precise across heterogeneous nondeterminism (rule ∨ match ∨
superpose ∨ let*), which a depth/stack-index cut cannot be (depth aliases across fan-out kinds;
the `fork_depth==0` single-rule shim is invisible to it — the exact cut.metta bug).

### The cut bug (proof)
`PeTTa/examples/cut.metta`: `(= (match-single $s $p $r) (let* (($x (match $s $p $r)) ($_ (cut))) $x))`,
`(foo $1)` matches `(foo 1)` and `(foo 2)`. PeTTa → `(bar 1)` (cut commits to first); MTT → `(bar 2)`.
Root cause: `match-single` fires via the single-rule fast path (`eval_loop.rs:902`) pushing a
`ProcessRuleMatches` shim with `fork_depth: 0`; the `let*` fans out via `ProcessAmb`
(`eval_loop.rs:16569`) which has NO cut check; `(cut)` sets `CUT_TARGET_DEPTH` from `FORK_DEPTH==0`;
consumption (`eval_loop.rs:7812`) requires `depth>0` → the signal is set but **structurally
unreachable** by the continuation that owns the alternatives.

### The substrate primitive
Replace `CUT_TARGET_DEPTH: Cell<u32>` with a monotonic barrier-id stack (thread-locals beside the
`dispatch_hints.rs:379` cluster):
```rust
static CUT_BARRIERS: RefCell<SmallVec<[BarrierFrame; 8]>>;   // preallocated, cap 8
static CUT_SIGNAL:   Cell<Option<BarrierId>>;                // 0 = none
static NEXT_BARRIER_ID: Cell<u64>;
struct BarrierFrame { id: BarrierId, kind: BarrierKind }     // Cut | Once | NafProbe | SoftCut
type BarrierId = u64;                                        // 8-byte Copy, monotonic
```
- A fan-out continuation that opens a cut scope allocates a `BarrierId`, pushes a `BarrierFrame`,
  and records the id in a new `cut_barrier: BarrierId` field on its struct (mark rides in the heap
  continuation — stack-safety mandate satisfied).
- `(cut)` sets `CUT_SIGNAL = Some(innermost enclosing barrier id)`.
- Every fan-out's "advance to next alternative" arm calls shared `cut_fired_for(self.cut_barrier)`:
  `CUT_SIGNAL == Some(b)` → clear + true. On true: drop `remaining_*`, `CP_TRAIL.undo_to(mark)`,
  resume with accumulated results. O(1)/check.
- **Scope binding:** a rule RHS lexically containing a reachable `(cut)` opens the barrier; compute
  `RuleEntry::body_contains_cut` at load time (`rule_management.rs`, mirroring `body_wants_lazy_args`)
  → zero runtime cost for the 99.9% cut-free rules.

Once this primitive exists, cut / `once` / NAF / soft-cut are thin layers over it.

## Trail re-scope (avoid the reverted contamination)
`tabling.rs:100-110` records that storing `(value, bindings)` in caches was REVERTED (commit
2e669c0) for within-query cross-caller contamination. So the union-find+trail `binding_store.rs`
(f917b1e) is NOT wired into cache-hit sites. It becomes the **mark/undo choice-point trail**:
`mark()` at fan-out push, `undo_to(mark)` on branch death/cut, clause-global binding propagation
for native conjunction (Phase 4) and match-pattern conjunction (Phase 6). The sidecar
`GenericBindings` stays the observable wire format (the `(Bindings …)` round-trip at
`eval_loop.rs:13780-13837`), materialized from the store at activation/output boundaries.

## Phase roadmap (smallest-first, each independently committable, benchmark-gated)
Each preserves: nextest 4215, conformance --strict 480/0/0/0, M11-pt 220, M11-he 40, canonical PLN
(5 examples + 7 ruletests); 20-run stability gate on every nondeterminism-affecting phase
(template `tests/ghost_branch_regression.rs:561`). Build cap 24G; nextest 96G + CPUQuota=1800%.

| # | phase | kind | gist |
|---|-------|------|------|
| 0 | choice-point trail wiring | infra | `CP_TRAIL` thread-local; install at activation boundary; inert (read by nothing); `with_capacity` preallocation; debug_assert snapshot ⊇ sidecar keys |
| 1 | **CUT correctness** | **bug-fix (must-pass)** | barrier-identity; `body_contains_cut`; `cut_barrier` on every fan-out; `cut_fired_for`; let* threads barrier; **update cut conformance fixtures to PeTTa-correct values** |
| 2 | NAF (`\+`) + `once` | expressiveness | NafProbe/Once barriers; `eval_naf_generic`/`eval_once_generic`; eval-position arms |
| 3 | soft-cut (`*->`) + K-Case | expressiveness | SoftCut barrier; all-Cond-solutions / Else-iff-zero; reuse project_alt_carrying:false |
| 4 | native conjunction `(, …)` | expressiveness (high-risk) | eval-position ONLY; parse/emit unchanged; gate on `lhs_head_all_meta_typed` (preserve `=>`); thread bindings via CP trail; anti-recurrence fixture |
| 5 | SLG tabling audit | expressiveness/perf | keep table & trail orthogonal; resolve-vars-before-`tabling_hash`; property test vs PeTTa answer sets; no full-SLG (no corpus demand) |
| 6 | match-pattern conjunction `[, P1 P2]` | expressiveness | conjunctive match-query over PathMap/MORK threading shared vars via CP trail; clause/2-as-introspection = read-only RuleIndex query (Prolog mutable clause/2 is host-FFI → out of scope, justified) |
| 7 | performance | perf | first-arg-index audit (RuleIndex.by_first_arg_head + PerHeadAtomIndex bloom == WAM first-arg indexing); preallocation; profiling-gated HAMT fork snapshots |

**Highest-value first phase: Phase 0** (inert foundation; de-risks the activation-boundary
save/restore — the one place that corrupts everything if wrong; 1-line rollback). The must-pass
bug-fix (cut) is **Phase 1**, depending on Phase 0's mark/undo for correct backtracking.

## Explicitly NOT built (justified, not skipped)
- Monolithic WAM choice-point stack + stack-index cut — does not map to the continuation forest;
  barrier-identity is strictly more expressive (justified by code structure).
- Binding store as a cache-hit binding cache — re-introduces the 2e669c0 contamination; motivating
  bug already fixed.
- Full SLG with answer subsumption — zero corpus demand; cycle-detect memo + resolve-before-hash
  captures the safe payoff.
- Prolog-level mutable `clause/2`/`assertz`/`retract` — host-FFI in PeTTa, absent from MeTTa corpus;
  engine analog (match-pattern conjunction + read-only RuleIndex query) is in-scope.
- CLP(FD) — host-dependent, no corpus demand.
- Native eval-position conjunction `(, …)` (was Phase 4) — empirically NOT needed: PeTTa returns
  `(, A B)` as DATA in eval position too (verified 2026-05-26), exactly like MTT. `(, …)` is only
  ever an `=>` antecedent (PLN path, works via foldl) or a `match` pattern (Phase 6, works). Building
  an eval-position handler would diverge from PeTTa. See the 2026-05-26 ledger entry.
- NAF `\+` and soft-cut `*->` (was Phase 2-rest / Phase 3) — ZERO corpus usage (`\+` absent; `*->`
  only INTERNAL to PeTTa's own translator `get-type`, never a user MeTTa form). Deferred as
  completeness-only; they are thin barrier layers (like `once`) buildable on demand if a corpus need
  appears. Match-pattern conjunction `[, …]` likewise: the `(, P1 P2)` match form (Phase 6) already
  works; no separate `[, …]` syntax appears in the corpus.

## Constraints (all phases)
Heap-trampolined (marks in heap continuations; iterative path-compressed `find`); PathMap/MORK/MM2
untouched (control-layer only); no env/CLI/pragma/feature behavioral gates; no MeTTa in built-ins;
`.expect("…")` not `unwrap()`; non-blocking/persistent where it aids parallelism; preallocate.

## Benchmark methodology (AMD Threadripper PRO 5975WX — 32c/64t, 4 CCD×8, 32 MiB L3/CCD, 128 GiB, perf governor)
- CPU-affinity per CCD (`taskset -c 0-7`) to isolate L3; 1-CCD vs 4-CCD scaling exposes cross-CCD
  L3-miss on the `results` mutex (`eval_loop.rs:2128`).
- `perf record --call-graph lbr` — confirm `undo_to`/`find`/`compose_outer_inner_strict_generic`
  not hot (the trail should REMOVE compose calls).
- `hyperfine --warmup 3 --runs 20` on: PLN-main suite, mmverify, conformance wall-time, cut/
  backtracking microbench. Assert Robot.metta peak RSS unchanged (the 219MB→900MB OOM class).

## Progress ledger
- 2026-05-26: design complete; tasks #34 (baseline)…#41 (perf) seeded. Implementation starting at Phase 0.
- 2026-05-26: **Phase 1 (cut barrier-identity) committed `1fcea16`** (impl by general-purpose
  agent, INDEPENDENTLY VERIFIED by supervisor). Replaces depth-cut with monotonic BarrierId
  threaded through every fan-out + cut-scope sequential veto in try_acquire_budget. VERIFIED:
  PeTTa cut.metta → (bar 1) ✅; rule-wrapped two-match cut (cut_seq2) → (pair 1 10) ✅; nextest
  4222; conformance --strict 481 / M11-pt 221 / M11-he 40; PLN suite ✅. PERF: Robot A/B 6.99s(P0)
  vs 7.19s(P1), CCD0-pinned = within noise (the earlier "10.6s" was measurement drift under
  concurrent builds — the cut-veto fires 0× for Robot, confirmed by probe). Agent MISREPORTED
  "cut_seq2 works" pre-verification (its inline form failed; rule-wrapped works) — supervisor
  re-derivation caught it. KNOWN REMAINING: **cut_nested** — a clause whose let* calls a SEPARATE
  cut-bearing rule then cuts commits its own multi-match to LAST (MTT) vs FIRST (PeTTa). Deep
  nested cut-scope ordering in let*/ProcessAmb; corpus-absent (cut.metta is the only corpus cut);
  real PeTTa divergence to close. Repro: /tmp/cnr.metta.
- 2026-05-26: **cut_nested RESOLVED** (the Phase 1 "KNOWN REMAINING" above is closed). Root cause
  was NOT a barrier-ordering flaw — it was **memoization of `(cut)`**. A barrier-lifecycle trace
  (probes on alloc/set/peek/consume/latch over /tmp/cnr.metta) showed the inner cut-rule's `(cut)`
  fired `set_cut_active` exactly ONCE, and the outer clause's textually-identical `(cut)` reached
  `ProcessLet($z)` with a value already in hand — **no second `eval_cut_generic` call**. The outer
  `(cut)` was served the inner one's memoized `(unit)`, skipping the `set_cut_active` side-effect, so
  the outer cut never pruned `$a` (committed to LAST). Fix: add `"cut"` to
  `dispatch_hints.rs::is_impure_head` — `should_memoize((cut))` now returns false, forcing every
  `(cut)` to re-evaluate. `expression_involves_cut_rules` only excluded RULES whose RHS contains
  cut (the bytecode-tier gate), never a bare `(cut)` from the memo cache. One-line semantic fix
  (cut IS impure — it mutates CUT_SIGNAL). VERIFIED: cut_nested `(pr 1 1)` ✅; cut.metta `(bar 1)` ✅;
  cut_seq2 `(pair 1 10)` ✅; `cut_barrier_regression` 8/8 (added `cut_nested_inner_cut_rule_not_memoized`
  + `cut_nested_deterministic_20_runs`); M11-pt 221 / M11-he 40; PLN 5/5 ✅ (Direct/Smokes/Toothbrush
  exact canonical strength-weighted values, FlyingRaven, Robot). **Phase 1 (cut) now COMPLETE — no
  known cut divergence.** (nextest + conformance --strict re-confirmation in progress.)
- 2026-05-26: **baseline @ 5637128** (CCD0 cores 0-7, perf governor, hyperfine -N --runs 8):
  PLN-Smokes 178.7 ms ± 5.0; PLN-Toothbrush 1.725 s ± 0.031; PLN-FlyingRaven 20.883 s ± 0.361
  (perf hotspot — Phase 7 target); mmverify-demo0 6.432 s ± 0.043 (User 3.9 s / **System 17.4 s**
  → allocation/syscall pressure); PLN-Robot 6.442 s ± 0.168 (4 runs). File: /tmp/wam_baseline_5637128.txt.
- 2026-05-26: **Phase 2 (`once`) COMPLETE** (data-driven re-prioritization: `once` has HIGH corpus
  demand — once/matchsingle/metta4_streams/invertpeanoplus/hyperpose_primes/tilepuzzle/matespace/
  plntestdirect — whereas NAF `\+` (Phase 2's other half) and soft-cut `*->` (Phase 3) have ZERO
  corpus usage, so NAF/soft-cut are deferred as completeness-only). `(once X)` ≡ Prolog
  `once(G)=(G,!)` scoped to G: commit X to its FIRST answer + PRUNE the rest (load-bearing for
  lazy/infinite X). Built as a thin layer over the Phase-1 cut barrier: `(once X)` desugars to the
  verified cut idiom `(prog1 X (cut))` = `(let $r X (let $_ (cut) $r))` under a FRESH barrier opened
  by a new `StartOnce` eval-step and owned by a new `ProcessOnceRestore` continuation (consume the
  once's cut signal + restore the enclosing barrier — the `is_barrier_owner` lifecycle). 6 edits:
  StartOnce (step/types.rs) · `"once"` arm w/ `freshening::allocate_epoch`+`intern_fresh_name`
  hygiene (step/sexpr.rs) · ProcessOnceRestore + 4 match arms (trampoline/types.rs +
  continuation_to_stack_symbol) · StartOnce/ProcessOnceRestore handlers (eval_loop.rs) · `"once"` in
  is_impure_head (never memoized — cut_nested lesson) · `"once"` → T0 in can_compile_with_env
  (the T1 VM has no `once` lowering — caught a real bug where `(collapse (once (xs)))` returned
  `((once 1) 2 3)` via the bytecode data-constructor fallthrough). `once` is SELF-BARRIERING: it
  opens its own barrier, so it commits even at top level (the bare cut idiom needs an enclosing rule
  barrier — once is strictly more self-contained). Scope-precision verified: `(once …)` in a let*
  does NOT prune a sibling `superpose`. VERIFIED: once.metta `(bar 1)` ✅; matchsingle (once≡cut) ✅;
  metta4_streams once-test `((num 1))` ✅ (lazy prune); nextest **4231/4231**; conformance --strict
  **481/481** (3 once fixtures 004-once/017-cut/401-cut-superpose-cross flipped petta-semantic-
  difference→same, now matching PeTTa); M11-pt 221 / M11-he 40; PLN 5/5; once_barrier_regression 7/7;
  cut 8/8. (Corpus files peano/invertpeanoplus/logicprogset/hyperpose_primes still fail on UNRELATED
  pre-existing gaps — foldall, `(plus $A $B)` reverse-unification, `and`-over-non-Bool, huge-number
  `>` overflow — NOT once; the once-specific sub-tests within them pass.)
- 2026-05-26: **Phase 4 (native conjunction `(, …)` in eval-position) — data-driven NOT NEEDED**
  (re-classified to "explicitly NOT built, justified"). Empirical finding: PeTTa does NOT resolve
  `(, A B)` as an eval-position goal either — `!(, (father tom $x) (father $x $y))` returns the
  `(, …)` form as DATA (freshened vars) in BOTH PeTTa AND MTT. The corpus uses `(, …)` ONLY as
  (a) `=>` macro antecedents (nars_direct, PLN Direct — PLN's path, already works via the foldl-atom
  desugar, 5/5 PLN), (b) `match` PATTERNS (matchnested2 — Phase 6), (c) nested inside other forms.
  None is eval-position goal-resolution. MTT already matches PeTTa (both return `(, …)` as data), so
  a `K-Conjunction` eval-position handler would be unobservable / divergent. The migration-plan's
  Phase 7 `K-Conjunction` premise rested on PLN's `=>` emitting native conjunction, which it does NOT
  (it desugars to foldl). Conclusion: do not build eval-position `(, …)`; the `,`-as-data behavior is
  correct and PeTTa-faithful.
- 2026-05-26: **Phase 6 (match-pattern conjunction `(match &self (, P1 P2) tmpl)`) — CORE ALREADY
  WORKS.** Verified: `(match &self (, (friend $1 $2) (friend $2 $3)) (transitive $1 $2 $3))` →
  `(transitive tim tom tam)`, matching the PeTTa oracle exactly (shared-var `$2` threaded across the
  conjuncts via the existing match machinery). Single-conjunct `(match &self (, (friend $1 $2)) …)`
  also works. So the conjunctive match-query over PathMap/MORK threading shared vars is already
  implemented — no new clause/2 engine needed. **However, matchnested2.metta STACK-OVERFLOWS** on a
  SEPARATE, pre-existing bug (NOT the conjunction): a `match` TEMPLATE that is a TUPLE headed by an
  SExpr with side-effects, e.g. `((add-atom &self …) (remove-atom &self …))`. Isolation matrix:
  single-side-effect template `(add-atom …)` / `(remove-atom …)` works (`[()]`); non-side-effect
  tuple template `((foo $1)(bar $2))` works; bare top-level tuple `((add-atom …)(remove-atom …))`
  works via outer-form-is-data; ONLY `match` + tuple-headed-by-SExpr + side-effects overflows. Tight
  2–3 frame recursion (gdb on stripped binary). NOT a regression from cut/once.
- 2026-05-26: **matchnested2 stack overflow FIXED `10d3cf5`** (stack-safety mandate). Root cause was
  NOT the conjunction NOR a true cycle — a **variable-hygiene name collision** causing ACYCLIC
  UNBOUNDED substitution growth. Proven by gdb (symbol build) + Python-DWARF value decode + a rename
  experiment: a rule whose LHS var collides by NAME with a free var in the matched argument (minimal
  repro `(= (id $z) $z)` + `!(id (wrap $z))`, independently reproduced) binds `$z → (wrap $z)`
  (self-referential by name); the transitive pre-resolution in `match_rules_native_inner`
  (rule_management.rs ~4010) substituted the inner `$z → (wrap $z)` without bound → overflow via the
  `apply_bindings_scoped ↔ apply_bindings_iterative` per-Spanned-form recursion (the key-freshening
  that disambiguates runs AFTER pre-resolution). Fix = a **self-exclusion guard**: a value containing
  its OWN key resolves against the OTHER bindings only (inner occurrence = caller's distinct var, stays
  free; key-freshening then disambiguates). STRICT no-op for non-self-referential bindings → zero PLN/
  dispatch regression (nextest 4235, --strict 481, M11-pt 221, M11-he 40, PLN 5/5 canonical).
  REJECTED the Plan agent's primary proposal (reorder freshening BEFORE pre-resolution) — it would
  break the legitimate transitive case `{$B → (Inheritance $1 …), $1 → Anna}` (the value's `$1` would
  no longer match the freshened key `$__fr_E_1`); caught by supervisor analysis. REMAINING (separate,
  tracked, NOT crashes): (1) matchnested2's FULL result needs the side-effecting-match-template
  ("SExpr-as-callable") semantics — `((add-atom…)(remove-atom…))` is treated as data so the transitive
  atoms aren't produced (returns `()`); documented design question. (2) Layer A (make `apply_bindings`
  Spanned-handling iterative via a `BuildSpanned` work item) — defense-in-depth stack-safety hardening
  for hypothetical deep nesting; deferred as a benchmarked change to the 9.5M-calls/run hot path (no
  corpus trigger once the unbounded growth is fixed).
- 2026-05-26: **Phase 7 profiling done; first optimization hypothesis REFUTED (scientific method).**
  `perf record --call-graph fp` on FlyingRaven (CCD0-pinned `taskset -c 0-7`, perf governor, 11k
  samples) flat self-time hotspots: `quicksort::<usize>` 5.9% · `collect_variables_generic` 5.4% ·
  `MettaValue::as_atom` 4.3% · **`expr_contains_cut` 4.1%** (Phase-1-introduced) · `as_sexpr` 3.2% ·
  `collect_subgoal_roots` 3.2% · libc memcpy/memset region ~20% (alloc/copy) · `Sip13 Hasher::write`
  2.4% · `SlabAllocator::alloc_data` 2.1% · `expression_involves_rule_rhs_atom` 1.9% ·
  `collect_thunk_roots` 1.7%. HYPOTHESIS: the #1 `quicksort::<usize>` is `incremental_gc.rs:437`
  re-sorting the already-sorted `live_ptrs_sorted` ∪ unsorted `remembered_set`; replace with an
  O(n+m) MERGE. Implemented + benchmarked (hyperfine -N -r 5, CCD0): BEFORE 19.848 s ± 0.059 vs AFTER
  19.940 s ± 0.073 — **within noise, NO improvement → hypothesis REFUTED, change REVERTED.** Root
  insight (scientific accuracy): the algorithmic O(n+m)-merge-vs-O((n+m)log)-sort win did NOT
  materialize because Rust's `sort_unstable` is **pdqsort**, which detects the already-sorted
  `live_ptrs_sorted` run + the tiny unsorted `remembered_set` tail and is ALREADY near-linear on this
  mostly-sorted union — so the merge had no real edge. The residual 5.9% flat-profile cost is inherent
  to TOUCHING that many `usize`s (the `extend_from_slice` copies + dedup + binary_search rebuild each
  collection), not the sort algorithm; reducing it needs FEWER roots / fewer mark-set rebuilds, a
  deeper change. (Workload is only weakly parallel — User 21.5 s / wall 19.8 s = 1.09× — so a
  "not-on-critical-path" explanation is secondary.) **LESSON (data-driven mandate): a flat self-time
  hotspot is necessary but not sufficient justification — always confirm with a before/after wall-clock
  benchmark, AND check whether the stdlib primitive (pdqsort here) already exploits the data shape.**
  The other flat hotspots (collect_variables, expr_contains_cut, memcpy) need the same before/after
  validation before any change.
- 2026-05-26: **Phase 5 (SLG tabling audit) COMPLETE — invariants confirmed, no full-SLG needed.**
  Audited the three Phase-5 correctness properties: (1) **resolve-before-hash**: SATISFIED by
  construction — `should_memoize` (dispatch_hints.rs:550) returns false for any expr where
  `has_variables_fast()`, so ONLY GROUND (fully-resolved, variable-free) expressions are ever
  tabled/memoized. There are no unresolved variables in any tabling key, so the alpha-equivalence /
  bound-var cross-contamination an SLG resolve-before-hash step guards against cannot arise (the
  ground-only gate is strictly stronger). (2) **table ⊥ trail**: the Phase-0 `CP_TRAIL` thread-local
  is referenced ONLY in eval_loop.rs (activation save/install/restore) and by NO tabling.rs / cache-hit
  site — orthogonal by construction, deliberately avoiding the reverted-2e669c0 (value,bindings)-in-
  cache contamination. (3) **cycle-detect, not full-SLG**: `cesk::tabling::ACTIVE_EVAL_SET`
  (refcount map) provides cycle detection via `is_actively_evaluating`; there is no answer-subsumption
  SLG (zero corpus demand). The one memoization-correctness GAP found this session — `(cut)` being
  memoized (skipping its side-effect) — was closed by adding `cut`/`once` to `is_impure_head`
  (82400d5 / 70db5d3). Values-only cache + re-tag-with-caller-bindings on hit (CompleteSubgoal)
  handles within-query isolation. Conclusion: tabling is correct under all three invariants; no change
  needed.

## Status summary (2026-05-26) — final
- **Phase 0** trail infra ✅ `691f226` · **Phase 1** cut (incl cut_nested) ✅ `1fcea16`+`82400d5` ·
  **Phase 2** `once` ✅ `70db5d3` · **Phase 3** NAF/soft-cut — **VACUOUS, justified NOT built**:
  PeTTa's translator exposes NO `naf`/`not`/`\+`/`*->` MeTTa surface form (dispatch list `HV == …`
  has cut/once/case/match/collapse/superpose/hyperpose/… only); `\+`/`*->` are INTERNAL Prolog
  constructs, never user MeTTa — there is no feature to build · **Phase 4** eval-position `(,)` —
  **justified NOT built**: PeTTa returns `(, A B)` as DATA in eval position too (verified) — an
  eval handler would diverge · **Phase 5** SLG audit ✅ invariants confirmed (ground-only hash,
  table⊥trail, cycle-detect) · **Phase 6** match-conjunction ✅ core works · **Phase 7** perf —
  profiled + baseline (FlyingRaven 19.848 s ± 0.059 @ d2b8a60, CCD0) + 1 hypothesis tested+refuted
  (GC merge; pdqsort already optimal); further optimization is open-ended and needs wall-clock-
  validated targets (the documented lesson) — `expr_contains_cut` (Phase-1 self-introduced) is the
  cleanest candidate if confirmed on the critical path (fix = thread cached `any_rule_body_contains_cut`
  flag). Plus discovered+fixed a GENERAL var-hygiene stack overflow `10d3cf5` + **Layer A** iterative
  Spanned-handling ✅ FULLY CLOSED in two parts: inner work-stack `BuildSpanned` `92d4f4e` (nested
  Spanned encountered while walking SExpr children) + ALL SIX outer-wrapper top-level peels
  de-recursed `2aac6dc` (`while`-loop peel + one bounded self-call on the non-Spanned core). NOTE:
  this corrects `92d4f4e`'s "non-lazy path done; lazy variant bounded → reasoned boundary" claim — it
  was inaccurate; `92d4f4e` made only the INNER path iterative, and BOTH the lazy and non-lazy OUTER
  wrappers (plus with_classes / with_rename / with_rename_scoped_maybe_lazy and
  `helpers.rs::apply_bindings`) still recursed once per outer span layer until `2aac6dc`. +2
  deep-Spanned (200k) no-overflow regression tests. (`types.rs::infer_types` Spanned/Lazy recursion
  scoped OUT — structurally recursive on SExpr depth anyway; separate broader concern.) Gate green
  throughout: nextest 4237 (4235 + 2 new), conformance --strict 481, M11-pt 221, M11-he 40, PLN 5/5
  canonical.
- 2026-05-26: **matchnested2 full-result = the outer-form-is-data vs reduce-all-elements FORK
  (PROVEN; genuine user design decision, not incompletion).** Bare-sequence test
  `!((add-atom &self (x 1)) (remove-atom &self (thing a)))`: PeTTa → `(true true)` (BOTH side effects
  run — reduces EVERY element of a tuple whose head is itself an SExpr); MTT → `((add-atom…) ())`
  (head-position SExpr kept as DATA — "outer-form-is-data", only non-head args reduced). So the
  matchnested2 `()` is NOT match-specific — it is MTT's DELIBERATE outer-form-is-data semantics
  (load-bearing: `can_compile_with_env`/`can_compile` route head-is-SExpr forms to T0 as structural
  data; PT-canonical; historical T04/020-061 cons/decons literal-structure tests depend on it). Adopting
  PeTTa's reduce-all-elements would break those tests — the documented SExpr-as-callable dilemma
  ([[session-handoff-2026-05-23-sexpr-callable]], 4 options A/B/C/D pending the user's choice). This
  is a core eval-semantics fork that requires the user's design intent; it is NOT a WAM-phase task and
  must not be changed unilaterally (would regress the gate).
- 2026-05-26: **User chose "adopt PeTTa reduce-all" → IMPLEMENTED `0448b52` (gate-safe), but
  matchnested2 still ❌ on a SECOND, deeper blocker: side-effect-commit through multi-element eval.**
  `EvalSExprTail` now evaluates EVERY element of an SExpr-headed tuple (head included) instead of
  pre-seeding the head verbatim — `!((add-atom…)(remove-atom…))` now reduces both elements (was
  `((add-atom…) ())`). VERIFIED SAFE: nextest 4235, --strict 481, M11-pt 221, M11-he 40, PLN 5/5
  (the historical no-flatten tests survive because they use NON-reducible head-SExprs that reduce to
  themselves; PLN `(? $term)` survives because `(grandfather a c)` has no directly-applicable rule).
  BUT matchnested2 still returns `()`: the side-effecting tuple elements now RUN (→ `()`), yet their
  `&self` SPACE MUTATIONS DO NOT COMMIT. Minimal repro: `!((add-atom &self (x 1)) (nop))` then
  `!(collapse (match &self (x $v) (x $v)))` → `()` (x NOT added), whereas STANDALONE
  `!(add-atom &self (y 2))` DOES commit. So the blocker is space-mutation propagation through the
  multi-element-eval path: `CollectSExpr` (eval_loop.rs:7614) → `process_collected_sexpr_generic`
  (processing/ops.rs:172) → `MettaEnvironment::union_all` (environment/core.rs:1386) → directive-env
  commit. Empirically RULED OUT the simple fix (threading the prior element's result env into the
  next element's dispatch — eval_loop.rs:7877 — did NOT make the mutation commit), so the gap is in
  `union_all`'s modified-detection / the COW space-layer model NOT merging a sub-eval's add-atom
  space additions up to the committed directive env (NOT just element-to-element threading). This is
  a deep, hot-path, broad-impact space-commit-machinery change (CollectSExpr is used by ALL
  multi-element forms) for ONE niche corpus example (match with a side-effecting tuple template +
  remove-during-match). Precisely localized; a careful, separately-benchmarked follow-on. reduce-all
  (the user's decided fork resolution) stands as a verified, PeTTa-faithful improvement.
  **[SUBSUMED 2026-05-26 by Gap A `6ccee22`.** This "follow-on" is now MOOT and was NOT needed:
  the global-atomspace change routes facts through in-place `add_to_space_shared` on the Arc-shared
  store, so a multi-element-eval add-atom commits without any `union_all` COW-merge fix. Verified:
  `!((add-atom &self (x 1)) (nop))` then `!(collapse (match &self (x $v) (x $v)))` → `[((x 1))]`
  (committed). No remaining task here.]
- 2026-05-26: **matchnested2 COMPLETE root-cause: blocked by TWO fundamental architecture forks
  (proven, gate-safe state restored).** After reduce-all (`0448b52`), an exhaustive empirical trace
  (each step gate-checked) localized the remaining failure to env/space-commit machinery, then proved
  it is NOT a localized bug but two core eval-strategy differences between MTT and PeTTa:
  (1) **Global atomspace vs COW per-branch env.** A side-effecting `match` template
  (`((add-atom &self (transitive …)) (remove-atom &self (friend …)) …)`) must have its `&self`
  mutations ACCUMULATE across match branches and COMMIT. The fix chain that made this work in
  isolation — `add_to_space` `mark_modified` + a flag-preserving clone + `CollectSExpr`/
  `ProcessMatchTemplates` env-THREADING (replacing `fork_for_nondeterminism`) — made
  `match`+tuple-template side effects commit (verified: `m_tuple`/`m_conj` → `((seen …))`/
  `((transitive tim tom tam))`), BUT REGRESSED 3 conformance tests (`M04-spaces/001-self-add-get`,
  `002-match-self`, `031-match`): threading destroys the match-branch binding ISOLATION those tests
  rely on. MTT's `&self` is COW (per-env, forked for nondeterminism); PeTTa/HE use a GLOBAL atomspace
  where mutations are inherently visible across branches while bindings stay branch-local. Matching
  PeTTa needs `&self` made globally-shared (e.g. `add_to_space_shared`-style in-place mutation of the
  shared atom_space) WITHOUT breaking COW binding isolation — a major env/space-model change. The
  threading approach was REVERTED (regressions); gate restored to 481.
  (2) **Eager vs lazy impure-arg eval.** matchnested2's directive 1 `(hide (tuple-of-add-atoms))` and
  even `(hide (add-atom &self (friend a b)))` must RUN the arg's side effects, but MTT is LAZY (the
  rule `(= (hide $1) (empty))` discards `$1` without forcing it, so the arg is never evaluated → no
  friends added → directive 2's match finds nothing). PeTTa is EAGER (args reduced before the rule
  applies). Proven: MTT `(hide (add-atom …))` → atom NOT added; PeTTa → added. Matching PeTTa needs
  eager evaluation of impure rule args (run side effects even when the body discards them) — a
  fundamental eval-strategy change (MTT is lazy by design).
  Both are MAJOR architectural forks (like the SExpr-as-callable fork the user decided as reduce-all),
  not localized bugs or deferrable patches; the localized threading patch provably trades one
  correctness property (side-effect commit) for another (branch isolation). matchnested2 is ONE niche
  corpus example (match with a side-effecting tuple template + remove-during-match + a lazy-discarding
  `hide` wrapper). Current state: reduce-all committed + gate-green; the two forks await a strategic
  decision (re-architect &self to a global atomspace + make impure args eager — a whole-evaluator
  change affecting PLN and all tests).
- 2026-05-26 (LATER): **matchnested2 RESOLVED end-to-end via the principled PeTTa-semantics
  redesign — both forks implemented, gate fully preserved.** The user authorized the redesign and
  reframed the risk decisively: *"With correct PeTTa semantics, PLN will not break"* — i.e. PLN is
  itself a PeTTa program, so a faithful global-atomspace + eager-impure-arg implementation cannot
  regress PLN; any regression would mean the implementation is wrong, not the semantics. A Plan agent
  designed the decoupling; the key precedent it surfaced: `fork_for_nondeterminism` (core.rs ~893)
  ALREADY `Arc::clone`s `rule_index` with the comment "rules added by one branch are visible to
  others" — the exact global-sharing pattern, just not yet applied to the atom store.
  **Gap A — global atomspace (`6ccee22`).** `Environment.atom_space` is now `Arc<AtomSpace<V>>`.
  `fork_for_nondeterminism` `Arc::clone`s it (nondeterministic branches SHARE the global store, like
  `rule_index`); explicit `clone()`/`make_owned` still DEEP-COPY (isolation preserved — this is what
  the 18 clone-isolation property tests assert). `union`/`union_all` wrap the merged store back in
  `Arc::new`. The `is_self_space` arms of `ProcessAddAtomSpace`/`ProcessRemoveAtomSpace`
  (eval_loop.rs) route FACTS through the new in-place `add_to_space_shared`/`remove_from_space_shared`
  (`&self`, mutates the shared store directly — visible across branches), while RULES/TYPES
  (`=`/`:`/`:<`) still go through COW `add_to_space`/`remove_from_space` (RuleIndex + types map).
  This gives PeTTa's "mutations visible across branches, bindings branch-local" without breaking COW
  binding isolation.
  **Gap B — eager impure args (`f004471`).** New `expr_has_space_side_effect` (grounded.rs,
  quote-aware SmallVec work-list over heads `add-atom`/`remove-atom`/`add-reduct`/`add-reducts`/
  `add-atoms`/`remove-all-atoms`). `find_grounded_arg_indices_generic` now ALSO forces an arg index
  when the arg has a space side effect — so `(hide (add-atom …))` runs the add even though the rule
  body discards `$1`. PURE args stay lazy (non-termination preserved); only space-mutating args
  become eager — the minimal faithful slice of PeTTa eagerness.
  **msort general-term sort (`5120dcc` T0, `2167a31` VM+JIT).** matchnested2's template sorts
  non-numeric tuples; MTT msort was numeric-only (errored). All three tiers now sort PeTTa standard
  order: numbers (f64) before non-numbers (by `friendly_repr`); numeric msort bit-identical.
  **Outcome.** matchnested2 PASSES: `is ((transitive sim som sam) (transitive tim tom tam)), should
  (…). ✅`. Gate fully preserved and re-verified after every step: nextest 4235/4235, mtt-conformance
  --strict 481/481, M11-bisimilarity-pt 221, M11-bisimilarity-he 40, PLN 5/5 canonical
  (Direct/Smokes/Toothbrush/FlyingRaven/Robot, exact strength-weighted values e.g. Direct
  `(stv 1.0 0.7290000000000001)`). The user's reframing held exactly: correct PeTTa semantics did not
  break PLN. **Empirical exoneration of the msort tier-consistency change:** the out-of-gate
  RavenInduction example produces BYTE-IDENTICAL output before (HEAD numeric-only msort, rebuilt and
  run) and after the VM/JIT change — its evidence bases are integer lists (numeric msort, unaffected);
  its ❌ is a pre-existing deeper PLN forward-chaining evidence-set gap (`(1 3)` vs `(1 2 3 4)`),
  unrelated to sorting and out of the canonical 5/5 gate.
- 2026-05-26 (LATER): **"Layer A" CLOSED — every `apply_bindings`-family Spanned-peel is now
  iterative (stack-safety mandate).** The prior entry's open item (2) "Layer A (make `apply_bindings`
  Spanned-handling iterative)" is resolved. An audit found the per-layer Spanned RECURSION was broader
  than the doc noted — SIX functions peeled spans recursively (one Rust frame per nested Spanned
  layer, i.e. an overflow on adversarially deep wrapper nesting):
  `helpers.rs:apply_bindings` (the non-generic Cow form), and in `bindings.rs`:
  `apply_bindings_scoped_generic`, `apply_bindings_lazy_scoped_generic`,
  `apply_bindings_with_classes_generic`, `apply_bindings_with_rename_generic`,
  `apply_bindings_with_rename_scoped_maybe_lazy`. (The inner bodies were already work-stack iterative
  via `BuildSpanned`; only these OUTER wrappers recursed. `engine.rs:apply_bindings` was already
  non-recursive — it delegates straight to the iterative `apply_bindings_inner`.)
  **Fix.** The five `bindings.rs` wrappers now peel all consecutive outer spans in a `while` loop
  (remembering the innermost span), dispatch the now-non-Spanned `core` through the SAME function (it
  cannot re-enter the span block → exactly one bounded self-call, preserving each function's post-span
  guards/inner), then re-apply only the innermost span unless the core result already carries one —
  semantically identical to the former per-layer recursion (incl. its per-level
  `result.span().is_some()` early-return) at every nesting depth, verified by tracing depths 0/1/N and
  the inner-already-Spanned case. `helpers.rs:apply_bindings` drops its redundant recursive Spanned
  arm entirely and routes Spanned through the existing work-stack `apply_bindings_iterative` (whose
  `BuildSpanned` produces byte-identical Cow results — inner-unchanged → `Borrowed(value)`,
  inner-changed → `Owned(Spanned(new_inner, span))`).
  **Perf:** the hot path (0 spans, ~9.5M calls/run) is byte-identical (the `if let Some(span)` guard
  is false and falls straight through to the iterative inner, exactly as before); the 1-span case is
  marginally cheaper (no recursive frame). So the deferral's perf concern does not arise.
  **Tests:** `apply_bindings_scoped_deeply_nested_spanned_no_overflow` +
  `apply_bindings_lazy_scoped_deeply_nested_spanned_no_overflow` build `Spanned^200000((foo $a))`
  (arena-allocated GC handles → bulk-freed, no recursive Drop) and substitute `$a→7`; the former
  recursion would overflow the ≈2 MB test-thread stack, the loop runs in constant stack.
  **Scope note (honest, not a deferral):** `eval/types.rs:infer_types_generic_inner` ALSO peels
  Spanned (and Lazy) recursively, but that function is STRUCTURALLY recursive on SExpr children
  (`infer_type_generic(&items[i])`, `infer_types_generic_inner(last)`) — i.e. it recurses on
  expression DEPTH regardless of wrappers. Its wrapper-peel arms are therefore not a separable
  "Layer A" item; making only them iterative would not make the function stack-safe. The whole
  type-inference path's structural recursion is a distinct, broader stack-safety concern (a separate
  work-stack rewrite of `infer_types`), explicitly out of the `apply_bindings`-Layer-A scope and
  recorded here rather than silently half-fixed.
