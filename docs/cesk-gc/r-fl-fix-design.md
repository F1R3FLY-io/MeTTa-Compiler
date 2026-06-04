# R-FL fix — idempotent free-list push (converged, red-teamed design)

**Date:** 2026-06-03
**Status:** IMPLEMENTED + scoped-verified. Production idempotent push in `add0585`; release-drain invariant in `ae61034`; index-gc integration test API fallout fixed in `70ab067`; bounded forced evaluator churn gate added in `tests/rfl_forced_gc.rs`, strengthened with minor/major counters in `d101e1b`, and repeated by `scripts/rfl_forced_gc_x20.sh` (`a017488`, path-derived in `3b7e645`). The post-fix FANOUT=0 ×20 gate is green; the dedicated-collector and pre-fix-bite gates remain separate (see "Remaining gate").
**Root cause:** `docs/cesk-gc/e1-flip-collapse-worker-env-gap-2026-06-03.md` (§"R-FL CONFIRMED").

## The bug (one line)

`IndexArena::sweep_range`'s free-list push is **non-idempotent**, and the current bump segment is **always young** (`promote_young` sets `young_floor = current_seg()` but the current segment stays young — the live alloc target), so **every** collection re-sweeps it; a dead slot not reused between two collections is pushed to `free_list` **twice** ⟹ two `pop_young_free_slot` pops hand the SAME slot to two `MettaValue`s ⟹ the first is clobbered ⟹ the ~3-5% wrong-subset corruption. Deterministically caught: `DUPLICATE FREE-LIST PUSH: addr=Addr(561) seg=0 off=561 sweep=minor`.

`promote_young` runs after **every** collection (`index_heap.rs:2114`, unconditional), and `cur_seg >= young_floor` always ⟹ **all** orderings duplicate: major→minor (observed), minor→minor, minor→major. Mode-INDEPENDENT (bug is in `sweep_range`, shared by the single-threaded FANOUT=0 collector and the dedicated FANOUT>0 collector). Conformance passes today only because its fixtures do 0 GC cycles (young < `YOUNG_BUDGET` = 2 MiB).

## Chosen fix: (A) idempotent free-list push via a persistent per-slot `free_bit`

Give each `Segment` a persistent word-packed `free_bit: Box<[AtomicU64]>` (exactly like `marks`, but **NOT cleared each cycle**), with the loop invariant **`free_bit(off) set ⟺ off ∈ free_list`**. At each push arm: `if seg.set_free_bit(off) { free_list.push(a) }` (skip if already set). On pop: clear the bit. On the major's rebuild: reset the bits of all listed entries **then** `clear()` (one inseparable operation). The duplicate becomes structurally impossible at the push site, regardless of sweep ordering.

### Why (A) beats the alternatives (red-team)
- **(B) exclude the current segment from the major's push — INSUFFICIENT.** `promote_young` runs after every collection ⟹ two consecutive *minors* both sweep the current segment ⟹ a **minor→minor duplicate** (B) never touches (it only changes the major). Rejected.
- **(C) promote the current segment too — UNSOUND.** Breaks the `pop_young_free_slot` cur_seg-only-reuse theorem (TLA-checked `StoreCentricGC_GenerationalYoungMark.tla`): advancing `young_floor` past `cur_seg` makes the just-promoted segment's free slots OLD ⟹ reusing one is the old→young UAF the design forbids. Plus segment churn. Rejected.
- **(F') minor rebuilds only the young free-partition — correct but dominated.** O(free_list)/minor (vs (A) O(1)/push), perturbs the load-bearing LIFO "cur_seg on top" property of `pop_young_free_slot`, and needs two cooperating code paths. Dominated by (A).
- **(A)** — O(1)/push, 1 bit/slot (= `marks` footprint), one local total invariant enforced identically at all three push arms, mode-independent. Chosen.

## Soundness
- **Eliminates every duplicate pattern:** `free_bit ⟺ on-list` is a lifecycle invariant (push sets, pop clears, drain resets-then-clears) ⟹ a second push of a listed slot finds the bit set and skips. Covers major→minor, minor→minor, minor→major. The legitimate **reuse-then-refree** cycle is preserved (pop clears the bit, so a later genuine re-free pushes correctly).
- **No leak:** the skip fires only when the slot is **already on the list** (already reclaimable); it removes a duplicate, never a reclamation. The major's reset-before-clear is **mandatory** — without it the post-clear rebuild would skip every slot (bits stale-set) and **leak the whole heap**.
- **cur_seg-only-reuse preserved:** (A) does not touch `young_floor`/`promote_young`/`pop_young_free_slot`'s cur_seg gate/`alloc`/segment lifecycle; it only changes *whether a slot is pushed twice* (orthogonal to the generational mark theorem). LIFO "cur_seg on top" preserved (no reordering; a suppressed push would have added a duplicate, removing it can't worsen the property).
- **Slab byte-identical:** `IndexArena`/`Segment` are constructed only via `global_index_heap`/`IndexFactory` (selected by `--features index-gc`); the slab build never constructs one ⟹ byte-identical by non-execution (same as `marks`/`swept`). Keep `free_bit` unconditional (like `marks`) — do NOT cfg-gate the field (avoids cfg-skew).
- **ABA/quiescence:** all free-list mutations run under the heap write lock at quiescence; the bitmap inherits the same total order. Use `AcqRel`/`Acquire` (parity with `marks`, forward-safe for the D-phase concurrent reader; byte-identical to `Relaxed` today under the write-lock).

## Exact edit sites (all in `src/backend/eval/cesk/index_arena.rs` unless noted)
- **E1** — `Segment` field: add `free_bit: Box<[AtomicU64]>` (word-packed, `ceil(capacity/64)` words, beside `marks` ~:280). Invariant documented; persistent (not cleared by `clear_marks`).
- **E2** — `Segment::new`: init `free_bit` to zeros (beside `marks` init ~:312), unconditional.
- **E3** — accessors beside `set_mark`/`is_marked` (~:386-405): `set_free_bit(off)->bool` (`fetch_or`, return was-not-set, AcqRel), `clear_free_bit(off)` (`fetch_and`, Release), `is_free_bit(off)->bool` (load & bit, Acquire). These supersede the detector's `AtomicBool` `mark/clear/is_on_freelist` at production density.
- **E4** — `Segment::release` (~:519-540): `self.free_bit = Box::new([])` (beside `marks`).
- **E5** — the three push arms (~:1287, :1321, :1364): replace `self.free_list.push(a)` with `if seg.set_free_bit(off) { self.free_list.push(a); }`. **`reclaimed_out.push(a)` stays UNGUARDED** (outside the `if`) — `free_reclaimed_side_slots` is idempotent on a re-reclaimed slot (`index_heap.rs:943-944`); guarding it would change `last_reclaimed` contents. Leave it exactly as-is.
- **E6** — `pop_young_free_slot` (~:767-792): after a successful `free_list.pop()` returning `addr`, `seg.clear_free_bit(addr.offset())` — for BOTH the `return Some(addr)` (cur_seg) AND the discard (non-cur_seg) branches, i.e. **before** the `if addr.segment() == cur` test (clear-before-branch — non-negotiable obligation #1).
- **E7** — the major drain (~:1203-1227): make the bit-reset + `clear()` one inseparable operation (reset every listed entry's `free_bit`, THEN `free_list.clear()`). **Reset-before-clear — non-negotiable obligation #2** (else heap leak). Keep the existing `!oracle` guard.
- **E8** — `sweep_young_with` doc-comment (~:1155-1166): the stated invariant *"a young slot is swept at most once before promotion"* is FALSE for the current segment — replace with *"the current segment is re-swept by every collection; duplicate free-list pushes are prevented by the per-slot `free_bit` (idempotent push), not by sweep-once."*
- **E9** — release-drain repair (post-implementation audit, `ae61034`): before releasing a fully-dead non-current segment, drain all existing free-list entries for that segment and clear the corresponding `free_bit` / debug shadow. This closes the D/TLAB fresh-bump interleaving where `alloc_bump` can advance `cur_seg` without consuming the prior young segment's listed free slots. Without E9, `Segment::release` drops `free_bits` while `free_list` still contains Addrs for that segment, violating `free_bit ⟺ on-list`.
- **NO change** to `promote_young`, the driver (`index_heap.rs`), the sweep signatures, or `SweepStats` (the decisive simplicity win — the fix is local to push/pop/drain + one field).

## Detector interaction (keep as a permanent cross-check)
Keep the committed `on_free_list` `AtomicBool` shadow + `freelist_check_enabled()` (env `METTATRON_INDEX_GC_FREELIST_CHECK=1`). **Repurpose** the three push-arm panics: they currently panic when `is_on_freelist` is set — after (A) that is the *expected* skip, so move the assertion to panic on **disagreement** between the production `free_bit` and the detector shadow (`seg.is_free_bit(off) != seg.is_on_freelist(off)`) at push/pop. Keep the pop "popped ⇒ was-listed" assert. Zero-cost when off; the standing oracle against a future re-regression of this subtle bug.

## Verification (conformance does 0 GC cycles — must FORCE GC)

### Completed scoped checks

- `METTATRON_INDEX_GC_FREELIST_CHECK=1 cargo test --lib --features index-gc index_arena::tests -- --nocapture`: passed, 23/23, including:
  - `repeated_current_segment_minor_does_not_duplicate_free_list_entry`
  - `minor_release_drains_listed_entries_for_released_segment`
- `cargo check --features index-gc`: passed after the source/TLA changes (warning baseline remains).
- TLC positive: `MC_RFL_freebit.cfg` passed with `NoDuplicateFreeListEntries` and `FreeBitExact`, including the release-drain transition.
- TLC negative: `MC_RFL_bug.cfg` still fails as expected with `freeList = <<0, 0>>`.
- Broad filtered compile/run after the `eval` return-shape fallout: `cargo test --features index-gc index_arena::tests -- --nocapture` passed after `70ab067`.
- Bounded evaluator churn gate: `systemd-run --user --scope -p MemoryMax=16G -p MemorySwapMax=0 -p CPUQuota=300% cargo test --test rfl_forced_gc --features index-gc -- --nocapture` passed, 1/1, 23.36s. The test enables `METTATRON_INDEX_GC_FREELIST_CHECK=1`, drives the public compile/eval path through generated allocation churn, and asserts `cycles_run()`, `minor_cycles_run()`, and `major_cycles_run()` all increase so the run is non-vacuous and covers both sweep kinds.
- Repeated FANOUT=0 post-fix gate: `scripts/rfl_forced_gc_x20.sh 20` passed under the capped lane, runs=20 failures=0, using the same detector-backed evaluator churn harness. The script derives the checkout path from its own location and uses `mktemp` for logs, so it is not tied to one local workspace path.

### Remaining gate

A bounded Rust harness now covers the post-fix FANOUT=0 evaluator-level non-vacuity check without relying on the earlier recursive MeTTa fixture. It builds+discards enough transient expressions to force both minor and major collection in every run; the post-fix ×20 arm is green. The **linchpin pre-fix bite** remains: the *pre-fix* binary should panic `DUPLICATE FREE-LIST PUSH` on the same detector-backed gate (reproduce first), and the *post-fix* binary should complete with 0 panics.

The current forced-MeTTa fixture attempt timed out rather than producing a clean gate result. Do not count V2/V3 below as fully discharged until the bounded deterministic harness also covers the dedicated-collector and robot-correctness cases.

| # | Check | Pass |
|---|---|---|
| V1 | detector ON, forced GC, FANOUT=0, ×20 | post-fix bounded evaluator churn passes ×20 with cycles>0, minors>0, majors>0; pre-fix-bite still pending |
| V2 | detector ON, forced GC, FANOUT=8 DEDICATED=1, ×20 | 0 duplicates (mode-independence) |
| V3 | robot correctness, FANOUT=0 AND FANOUT=8 DEDICATED=1, forced GC | wrong-subset rate 0% (was ~3-5%) both modes |
| V4 | conformance, slab AND index-gc, FANOUT=0 | 483/0 both (functional + slab parity) |
| V5 | ASAN, forced GC, FANOUT>0 | 0 UAF |
| V6 | 20-run determinism (`scripts/drlock_determinism.sh`), both modes | 1 distinct hash each |
| V7 | memory sanity (detector off) | +1 bit/slot (= marks), no other growth |

## Follow-up (noted, not v1)
The major drain's O(free_list) snapshot-reset can become an O(1) `free_epoch` version-counter (`free_bit` stores the epoch it was set; "on-list" ⟺ `epoch == current`) *if* major-drain ever shows in a profile. Keep the snapshot form for v1 (reuses proven detector code, correct-by-inspection).
