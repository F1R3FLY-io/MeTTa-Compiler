# Phase C Increment D — abstract-GC live-variable marking (Might–Shivers over reified K-frames)

**Status: APPROVED (user chose "implement D fully with the discharge"), executing.** HEAD `678d531`.
Source-verified Plan-agent design correcting the baseline D.1-D.8 (`phase-c-remaining-implementation-plan.md`).

## Crux findings (all verified at file:line)
1. **D is MIDLOOP-ONLY.** Quiescence collectors use `collect_persistent_roots` (roots.rs:351 = E₀ ∪ anchors ∪
   k_spine) — NO `collect_from_continuations`; `assert_quiescence_superset` (roots.rs:379) builds from
   persistent only (roots.rs:410, comment :407-408 "C∪K are empty at quiescence"). The K-walk
   (`collect_from_continuations`, roots.rs:183) is reached ONLY via `collect_machine_roots` (roots.rs:328)
   at the ONE midloop root-build (eval_loop.rs:3799-3806). So D narrows ONLY the midloop collector
   (`gate_open_midloop`, `METTATRON_INDEX_GC_MIDLOOP=1`, default-OFF). The shipped quiescence path is
   untouched ⇒ byte-identical green-wall. The discharge MUST be a MIDLOOP oracle + MIDLOOP ASAN.
2. **Deadness predicate = `cut_fired_peek(cut_barrier)`** (eval_loop.rs:2165 = `b != 0 && CUT_SIGNAL==b`;
   thread-local `CUT_SIGNAL` :1801, accessible+consistent on the sole eval thread at the midloop safepoint).
   It ALREADY encodes `b != 0` (drop the baseline's redundant extra check).
3. **ProcessGroundedOpFanout is MOOT** — no `cut_barrier`, never peeks (eval_loop.rs:8991-9053);
   `remaining_alts` live when non-empty, empty=no-op. Dropped from scope (baseline error).

## Corrected scope: EXACTLY 3 narrowing variants (baseline missed 2)
Verified by grepping every `cut_fired_peek` call site (eval_loop.rs:8506, 14734, 15578) — the
`cut_fired_peek(b) ⇒ drop(iter)` pattern:
| Variant | Field dropped on cut | Advance arm (proves dead) | collect_values arm |
|---|---|---|---|
| `ProcessRuleMatches` (types.rs:508) | `remaining_matches` (:509) | eval_loop.rs:8506-8534 (drop on commit) | types.rs:1863-1881 |
| `ProcessAmb` (types.rs:2297) | `remaining_alts` | eval_loop.rs:14734 (`drop(remaining_alts)`) | types.rs:2303-2311 |
| `ProcessMatchTemplates` (types.rs:~1560) | `remaining_templates` | eval_loop.rs:15578 (`drop(remaining_templates)`) | types.rs:2467-2478 |
NOT narrowable (carry `cut_barrier` but no droppable-on-cut iterator): `ProcessConjunction` (sequences,
doesn't abort on cut, eval_loop.rs:13167), `ProcessMatchSpace` (threads barrier to child), `CollectFreezeArgs`
(barrier=0), `ProcessRuleMatchesLazy` (cut is internal to BranchCoroutine cursor). → delegate to `collect_values`.

## A. `Continuation::collect_live_values(&self, out)` (types.rs, beside :1842)
A SMALL match: the 3 narrowed arms + `_ => self.collect_values(out)` (conservative default — every other +
future variant gets the full walk for free, can never silently under-root). Each narrowed arm: when
`!cut_fired_peek(*cut_barrier)` it is BYTE-IDENTICAL to that variant's `collect_values` arm; when fired it
SKIPS the dead iterator field and retains all else (`results`/`outer_carrying`/`current_branch_bindings`,
which the commit branch still reads). ⇒ `collect_live_values ⊆ collect_values` always; equality when no cut.
**Soundness obligation per dropped field:** the fired predicate forces the next transition into the drop
branch (no other reader); `cut_fired_peek` can't flip false before then (consumed only downstream by the
barrier owner). Visibility: `cut_fired_peek` → `pub(crate)` (eval_loop.rs:2165); add `#[cfg(test)]
pub(crate) fn force_cut_signal_for_test(b)` (test drives the thread-local).

## B. Wiring — MIDLOOP-only, narrow the single root vec (NOT minor-vs-major at the root-build)
`mark_sweep_if_over_watermark` decides minor-vs-major INTERNALLY (index_heap.rs:1421) from ONE `addrs`
projected from ONE `roots` vec (:1426) — so the root-build can't pick `_live`-vs-full by collection type.
RESOLUTION: narrow the single MIDLOOP root vec; BOTH the midloop minor and the (rare) midloop major mark
from it — SOUND for both (the narrowing is a machine-state property: a post-cut `remaining_matches` is dead
regardless of collection type; the young-only-mark theorem is untouched — D shrinks the root set, not the
descent). Edits (D-2): NEW `RootSet::collect_from_continuations_live` (roots.rs:183), `collect_all_live`
(:199), `collect_machine_roots_live` (:314); flip the ONE site eval_loop.rs:3799
`collect_machine_roots`→`collect_machine_roots_live`. Quiescence build, the A4 oracle's full-build, and
mark/sweep — ALL untouched.

## C. Mechanical soundness discharge (3 layers; NO new TLA+)
- **(1) Per-variant differential unit tests** (types.rs `#[cfg(test)]`, 3 variants × {fired, not-fired} +
  the barrier=0 edge): not-fired ⇒ `collect_live_values == collect_values` (multiset of `inner_ptr`);
  fired ⇒ `⊆` AND the dropped field's addrs absent AND all other fields present. Deterministic, no GC.
- **(2) NEW `assert_midloop_live_superset`** (roots.rs:379-sibling, `#[cfg(debug_assertions)]`, gated on
  `gate_open_midloop`): a REACHABILITY-CLOSURE check (NOT a raw root ⊆ — D deliberately makes narrowed ⊊
  full): `reachable(narrowed) ⊇ reachable(full)` via a borrow-only `dry_mark` into a scratch set; panic if a
  full-reachable value is unreachable from the narrowed roots. The quiescence oracle is INSUFFICIENT (K
  empty). Standing CI: `scripts/d_midloop_oracle.sh` (DEBUG index-gc, 483 + PLN + cut fixtures,
  `MIDLOOP=1 MIN_BYTES=1`) → 0 panics.
- **(3) MIDLOOP ASAN** (`scripts/d_midloop_asan.sh`, `cut_young.metta`): A's side-free is OFF at midloop
  (index_heap.rs:1464), so a wrong-narrow ⇒ `sweep_young` reclaims a live young NODE slot ⇒ ASAN
  heap-use-after-free, UNMASKED by the side-free. `MIDLOOP=1 MIN_BYTES=1GiB` (minors, no major preempt) →
  0 UAF + `midloop minor cycle`>0 (non-vacuous) + correct result; + a MIDLOOP-off control arm.
- **(4) TLA+: NONE.** D is a finite per-variant STATE predicate (cut⇒drop one iter), no new transition/
  concurrency behavior; the mark/sweep/boundary/gates/trigger are all already covered + unchanged. The
  differential tests + midloop oracle + ASAN fully discharge it.

## Gate (D-2): green-wall byte-identical (483/0, slab/index nextest +3 tests, conf cycles unchanged,
quiescence oracle 0) + differential tests + midloop oracle 0-panic over corpus + midloop ASAN 0-UAF
(non-vacuous) + 20-run + mmverify. **Throughput: ~zero (D inert on shipped path; opt-in midloop-only;
value = the genuine-CESK form + midloop marking precision, NOT perf — state plainly).**

## Commit plan
- **D-1**: `cut_fired_peek`→pub(crate) + `force_cut_signal_for_test` + `collect_live_values` (3 arms + `_`)
  + 3 differential tests. UNWIRED ⇒ byte-identical green-wall + tests green.
- **D-2**: `collect_from_continuations_live`/`collect_all_live`/`collect_machine_roots_live` + flip
  eval_loop.rs:3799 + `assert_midloop_live_superset` + `cut_young.metta` + `d_midloop_{oracle,asan}.sh`
  → full gate.
