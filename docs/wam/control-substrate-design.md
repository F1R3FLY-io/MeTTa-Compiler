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
