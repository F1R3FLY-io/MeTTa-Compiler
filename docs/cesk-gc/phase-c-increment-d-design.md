# Phase C Increment D — abstract-GC live-variable marking (Might–Shivers over reified K-frames)

**Status: APPROVED (user chose "implement D fully with the discharge"), D-2 executing.** D-1 committed `49e09fd`.
Source-verified Plan-agent design correcting the baseline D.1-D.8 (`phase-c-remaining-implementation-plan.md`).

**D-2 ORACLE DROP-AND-REPLACE (user-directed: "Use a plan agent for the principled solution").** The
discharge layer (2) below — the proposed `assert_midloop_live_superset` reachability-closure oracle
`reachable(narrowed) ⊇ reachable(full)` — was found **UNSOUND AS A CHECK** by a Plan agent: a legitimate
DEAD-drop and a buggy LIVE-drop shrink the reachable closure *identically*, so the check cannot distinguish
them and false-fails on **every** legitimate narrowing. The Plan agent's principled resolution = **(b)
drop-and-replace**: DROP the closure oracle; REPLACE it with **READ-SITE coupling `debug_assert`s** at the
three advance arms (the dangerous direction — `collect_live_values` skipped field F, yet the next transition
reads F). This catches the actual UAF precursor directly; the closure oracle could only catch the harmless
over-rooting direction. Section C below is rewritten accordingly.

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

## C. Mechanical soundness discharge (3 layers; NO new TLA+) — oracle DROPPED-AND-REPLACED
The soundness obligation per narrowed field F: **`collect_live_values` skips F ⟹ the next machine
transition does not read F** (so F is genuinely dead and reclaiming its storage is safe). This is
discharged by (1)∧(2): (1) proves the SKIP side (skip ⟺ cut fired for F's barrier); (2) proves the
READ side (the advance arm reads F only when the cut has NOT fired). Their conjunction is exactly the
obligation: skip ⟹ cut-fired ⟹ not-read. (3) is the empirical dynamic confirmation.

- **(1) Per-variant differential unit tests** (types.rs `#[cfg(test)]`, 3 variants × {fired, not-fired} +
  the `cut_barrier==0` edge): not-fired ⇒ `collect_live_values == collect_values`; fired ⇒ the dropped
  field's values are absent AND `results`/`outer_carrying`/`current_branch_bindings` are present (so
  `collect_live_values ⊆ collect_values`, equality iff no cut). `cut_barrier==0` ⇒ never narrows even with
  the signal forced nonzero (the `b != 0` guard, eval_loop.rs:2166). Deterministic, no GC.
  *Implemented:* `collect_live_values_narrows_process_{rule_matches,amb,match_templates}_on_cut` +
  `collect_live_values_barrier_zero_never_narrows` (types.rs `mod tests`); `force_cut_signal_for_test`
  drives the thread-local `CUT_SIGNAL`.
- **(2) READ-SITE coupling `debug_assert!`s** — *REPLACES the dropped reachability-closure oracle.* At each
  of the three advance arms, immediately before the arm pulls the next item from the narrowable iterator,
  assert the cut has NOT fired for the frame's barrier:
  - `ProcessRuleMatches`: `debug_assert!(!cut_fired, …)` before `remaining_matches.next()` (eval_loop.rs ~8556;
    `cut_fired` is the arm's own already-computed `cut_fired_peek(cut_barrier)`).
  - `ProcessAmb`: `debug_assert!(!cut_fired_peek(cut_barrier), …)` before `remaining_alts.next()` (~14762).
  - `ProcessMatchTemplates`: `debug_assert!(!cut_fired_peek(cut_barrier), …)` before
    `remaining_templates.next()` (~15611).
  These are TRIPWIRES for the dangerous direction: if a future edit ever lets an advance arm read a field
  that `collect_live_values` would skip, the assert fires under `cargo test`/DEBUG corpus runs. **Why the
  closure oracle was dropped (not implemented):** `reachable(narrowed) ⊇ reachable(full)` cannot be a valid
  check — a legitimate dead-drop and a buggy live-drop reduce the closure *identically*, so it false-fails
  on every legitimate narrowing (D deliberately makes `narrowed ⊊ full`). The read-site coupling catches the
  precursor to the actual UAF (skip-but-read); the closure oracle could only catch the harmless
  over-rooting direction. Standing CI: a DEBUG `--features index-gc` corpus run (`MIDLOOP=1 MIN_BYTES=1`)
  exercises all corpus cut paths against these asserts → 0 panics.
- **(3) MIDLOOP ASAN** (`scripts/d_midloop_asan.sh`, `cut_young.metta`): the dynamic confirmation that the
  REAL (correct) narrowing has no UAF under cuts + a midloop MINOR. The side-free is quiescence-gated (off at
  midloop), so a wrong-narrow's UAF surfaces via young-**segment release** of a wrong-narrowed live node
  (a real backing-Box free) — `cut_young.metta` commits all three narrowed K-frames (multi-clause `pick` =
  ProcessRuleMatches, the `let*` value fan-out = ProcessAmb, the `match` template fan-out =
  ProcessMatchTemplates) then drives POST-CUT young churn so a wrongly-reclaimed slot is reused/overwritten
  and a live ref dereferenced. `MIDLOOP=1 MIN_BYTES` high → 0 UAF + `midloop minor cycle`>0 (non-vacuous) +
  result `[done]`; + a MIDLOOP-off control arm (same `[done]` ⇒ the result is independent of midloop
  collection). RESULT (2026-06-01): both arms 0 UAF, 1 minor (non-vacuous), 0 read-site-assert panics.
  **Midloop MAJOR coverage (no separate ASAN needed, and structurally precluded here).** The narrowed root
  vec feeds BOTH the midloop minor and major mark. `cut_young`'s heap is young-only and ~7 MiB committed, so
  `CAP_FLOOR` (8 MiB) structurally precludes a midloop major on it (verified: MIN_BYTES=131072 still fires a
  minor, 0 majors), and a >8 MiB-committed cut workload is impractical within the loop+wrap ≲ 800 stack bound
  (the `(cut)` defeats loop TCO). This is sound, not a gap: **D narrows only the ROOT SET** — a
  collection-type-INDEPENDENT deadness property (a post-cut `remaining_*` field is dead whether a minor or a
  major would process it), discharged by (1)∧(2) WITHOUT reference to GC. It is strictly upstream of the
  mark-DESCENT (minor-stops-at-old vs major-marks-all), which is itself already verified by the C minor/major
  tests + the young-only-mark theorem and is UNCHANGED by D. A midloop major would therefore reclaim only the
  SAME proven-dead fields; it introduces no new soundness obligation beyond the deadness (1)∧(2) discharge.
- **(4) TLA+: NONE.** D is a finite per-variant STATE predicate (cut⇒drop one iter), no new transition/
  concurrency behavior; the mark/sweep/boundary/gates/trigger are all already covered + unchanged. The
  differential tests + read-site coupling asserts + midloop ASAN fully discharge it.

## Gate (D-2): green-wall byte-identical (483/0, slab/index nextest with the 4 new differential tests, conf
cycles unchanged, quiescence oracle 0) + the 4 differential tests green + DEBUG corpus run (`MIDLOOP=1
MIN_BYTES=1`) 0 read-site-assert panics + midloop ASAN 0-UAF (non-vacuous, `cut_young.metta`) + 20-run +
mmverify. **Throughput: ~zero (D inert on shipped path; opt-in midloop-only; value = the genuine-CESK form
+ midloop marking precision, NOT perf — state plainly).**

## Commit plan
- **D-1** (committed `49e09fd`): `cut_fired_peek`→pub(crate) + `force_cut_signal_for_test` +
  `collect_live_values` (3 arms + `_`) + 1 differential test (ProcessRuleMatches). UNWIRED ⇒ byte-identical
  green-wall + tests green.
- **D-2**: `collect_from_continuations_live`/`collect_all_live`/`collect_machine_roots_live` (roots.rs) +
  flip the ONE midloop feed `collect_machine_roots`→`collect_machine_roots_live` (eval_loop.rs ~3808; the
  A4.3 oracle build at ~3697 stays FULL) + the 3 read-site coupling `debug_assert!`s (the dropped oracle's
  replacement) + 3 more differential tests (ProcessAmb, ProcessMatchTemplates, `cut_barrier==0` edge) +
  `cut_young.metta` + `scripts/d_midloop_asan.sh` → full gate. **No `assert_midloop_live_superset`, no
  `d_midloop_oracle.sh`** — the closure oracle was dropped as unsound (§C).
