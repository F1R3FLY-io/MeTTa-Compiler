# ACT Out-of-Core Persistence (Stage 5a)

**Status: COMPLETE + validated.** The user authorized "build the true MM2 join first, then
wire the full MeTTa surface," and subsequently directed that the initially-deferred gaps be
closed with no deferrals. All of that is implemented and gate-green:

- **Part A — true MM2 `ProductZipper` join over ACT** (`MORK Space::<()>::query_multi_act` +
  MeTTaTron `query_act`).
- **Part B — full MeTTa surface** (`(save-space! …)`, `(load-space! …)`, `(query-act …)`).
- **Completeness (deferrals closed):** bag-faithful query, `wide_btm` (arity ≥ 64) save/load/query,
  and genuine **cross-run** persistence via a serialized symbol mapping.
- **The LSM tiered ACT-backed mutable space (§4) — DELIVERED** (was "next layer"): an immutable ACT
  base + mutable in-memory overlay + per-key tombstone suppression + `(compact-space!)` compaction,
  making ACT the *primary* store of a live, mutable space. `match_space` =
  `overlay ++ (base − tombstones)`, gated so the no-base hot path is byte-identical. Surface:
  `(attach-act-base! …)`, `(detach-act-base!)`, `(compact-space! …)`.

Gate (after each phase): `cargo nextest` **4282** (was 4252; +30 tiered tests), `mtt-conformance
--strict` **483/483**, PLN-main 7/7 (0 ❌). Tests: 8 Rust-API (`act_persistence::tests`) + 6 surface
(`tests/act_surface.rs`) for the snapshot/query layer; **19 Rust-API (`act_tiered::tests`) + 9
surface (`tests/act_tiered_surface.rs`)** for the LSM tiered layer.

This document is self-contained: the design can be reconstructed from scratch from it.

---

## 1. What and why

`ArenaCompactTree` (ACT, `PathMap/src/arena_compact.rs`) is a read-optimized, contiguous,
memory-mappable serialization of a PathMap trie. The goal (the user's "scalable architecture" /
"exploit MM2 for MORK queries" lever) is **out-of-core querying**: hold a fact set as a
file-backed mmap rather than in the heap, so a dataset larger than RAM can be queried with the OS
paging in only what a query touches. The atom space stays PathMap/MORK/MM2-backed — an ACT is a
*serialization of the same tries*, not a new store.

---

## 2. Delivered subsystem

### 2.1 Rust API (`src/backend/environment/act_persistence.rs`)

| op | what it writes/reads | semantics |
|---|---|---|
| `save_space_to_act(name)` | `<name>.act` (btm), `<name>.wide.act` (wide_btm, if any), `<name>.sm` (symbol mapping) | snapshot the whole space; multiplicity in each ACT's `u64` leaf |
| `query_act(name, pattern, template)` | reads the above | **out-of-core**, **bag-faithful** query: same multiset as in-memory `match_space` |
| `load_space_from_act(name)` | reads the above | restore the snapshot into the live space (Σ-multiplicity count) |

- **btm** (arity < 64) holds facts + rules `(= …)` + type assertions `(: …)`; on load,
  `add_to_space` re-routes each and rebuilds the derived type caches.
- **wide_btm** (arity ≥ 64) uses the self-describing Wide MORK encoding — sm-independent.
- Multiplicity rides in the ACT `u64` leaf (`dump_from_zipper`'s `map_val = |m| m.0`;
  `read_zipper_u64` reads it back). `query_act` is bag-faithful: the scan reads the leaf
  multiplicity directly; the join recovers it via an O(depth) `u64`-zipper descend to the
  matched key (`act_leaf_multiplicity`).

### 2.2 MORK addition (authorized): `Space::<()>::query_multi_act` (`MORK/kernel/src/space.rs`)

The ACT analogue of `query_multi`: a `ProductZipperG` over the mmap'd ACT's read-zippers (one per
conjunct of `(, g1 …)`), running the existing `query_multi_raw`. Trie-pruned (O(matches)). Takes
the (interned) conjunct pattern **directly** — no inline `(ACT name)` markers (see §3). Bindings at
namespace 0, as for `query_multi`.

### 2.3 MeTTa surface (T0 special forms, `src/backend/eval/step/sexpr.rs`)

`(save-space! "name")` → path String · `(load-space! "name")` → Long count ·
`(query-act "name" <pattern> <template>)` → superposed matches. Classified **impure**
(`dispatch_hints.rs::is_impure_head`, never memoized) and **T0-only**
(`bytecode/mod.rs::can_compile_with_env => false`). `<name>` is a literal String/Atom.

### 2.4 Out-of-core

`query_act` never holds the whole KB in the heap: the fact set lives in the file-backed mmap
(reclaimable page cache); the join/scan materializes at most one fact as a `MettaValue` at a time.

### 2.5 Cross-run soundness

`btm` keys reference the *saving* env's `SharedMapping`. `save_space_to_act` serializes it to
`<name>.sm` (`mork_interning`'s `SharedMapping::serialize`); `query_act`/`load_space_from_act`
deserialize it (`act_sm_for`) to **decode**, so a snapshot decodes in a different process run.
The trie-pruned join also *encodes* with the env's mapping — the intra-run fast path; on a miss
(genuine, or a cross-run mapping mismatch) `query_act` falls back to the sm-faithful scan, correct
either way. Wide facts are sm-independent. Validated by
`act_btm_round_trips_cross_environment_via_saved_sm` (a fresh env decodes another env's snapshot).

### 2.6 Validation

- `act_persistence::tests` (8): in-memory `match_space` equality; template projection;
  bag-faithful multiplicity > 1; save/load multiplicity round-trip; no-match empty; wide-arity
  (≥ 64) round-trip; cross-environment (cross-run) decode via the saved `sm`.
- `tests/act_surface.rs` (6): the three surface forms through the real evaluator.

---

## 3. Interning and why `query_multi_act` (not `query_multi_i`)

`query_multi_i`'s `(I (ACT name pat))` source form dispatches in `ASource::new` by byte-matching
**inline** symbol markers (`[Arity(3)][SymbolSize(3)]ACT`). MeTTaTron builds MORK with the
**`interning`** feature, so markers are interned IDs, not inline bytes — the dispatch falls into
`unreachable!()` (`sources.rs:313`, verified empirically). `query_multi_act` sidesteps this: it
takes the (interned) conjunct pattern directly and builds the `ProductZipperG` over the ACT itself
— no inline markers. The interned conjunct matches the interned ACT facts; bindings come back at
namespace 0.

---

## 4. LSM tiered ACT-backed mutable space (DELIVERED)

**An immutable ACT base + a mutable in-memory overlay (`btm`/`wide_btm`) + per-key tombstone
suppression + compaction**, so a *live, mutable* space is transparently backed by an out-of-core
ACT base — ACT as the *primary* store, not just an explicit snapshot/query target.

Implemented in `src/backend/environment/act_tiered.rs` (the tiered read + tombstone bookkeeping +
compaction) with three new fields on `AtomSpace<V>` (`src/backend/environment/atom_space.rs`):

```text
  act_base:    RwLock<Option<Arc<ActBase>>>   // attached immutable base (None = untiered)
  tombstones:  RwLock<PathMap<Multiplicity>>  // per-base-key SUPPRESSION COUNT
  has_act_base: AtomicBool                     // fast no-base gate (relaxed-acquire load)
```

### 4.1 Data model

- **base** (`ActBase`): an opened `<name>.act` (+ optional `<name>.wide.act`). `ActBase` stores
  **only** the base name + the resolved symbol mapping/epoch — NOT the `ACTMmap`. `ACTMmap`
  (`= ArenaCompactTree<Mmap>`) carries an interior `Cell<u64>` scratch slot and is therefore `Send`
  but **`!Sync`**; `AtomSpace<V>` lives behind an `Arc` shared across eval threads
  (`SharedEnv = Arc<GenericEnvironmentShared>`) and must be `Send + Sync`, so a cached `ACTMmap`
  would poison it. The tiered read re-opens the mmap per query via `ArenaCompactTree::open_mmap`,
  exactly as `query_act` does (the OS page cache keeps it warm). *(Deviation from the original
  design's "`ActBase { tree: ACTMmap, … }`, ACTMmap is Send+Sync" — that auto-trait claim is false;
  re-open-per-read is the sound, proven pattern.)*
- **overlay**: the existing `btm`/`wide_btm`/`variable_atoms` — every `add-atom` writes here.
- **tombstones**: a SEPARATE `PathMap<Multiplicity>` mapping a base-`sm`-encoded fact key → the
  COUNT of base copies to suppress. Separate from `btm` because the overlay's `remove_atom`
  auto-prunes 0-count entries — a tombstone must persist a *positive* suppression count. *(Deviation
  from the original "multiplicity-0 tombstones" phrasing: a count-of-copies-to-suppress, not a
  0-valued entry, for exactly that pruning reason.)*

### 4.2 Read — `overlay ++ (base − tombstones)`

`match_space` / `match_space_exists` / `match_space_first` / `match_space_query_multi` gate on
`has_act_base` (a relaxed-acquire load on the bloom-miss / end-of-overlay branch only, so the
**no-base hot path is byte-identical** — the Hard Constraint). When a base is attached they append
the base half via `match_space_base`, which reuses `query_act`'s two paths against the re-opened
mmap — the trie-pruned `query_multi_act` ProductZipper join for head-shaped arity-1..64 patterns,
else a leaf scan — and filters each matched fact's stored multiplicity by its tombstone count
(`effective = max(0, base_leaf_mult − tombstone(key_bytes))`), preserving bag multiplicity,
overlay-first. The base read is fully generic (`mork_bindings_to_generic` / `mork_bytes_to_generic_value`),
so it serves both the generic `match_space` and the `MettaEnvironment` entry points. The overlay
bloom early-return is bypassed when `has_act_base` (the bloom tracks only overlay heads; a base may
match a head the overlay never saw).

### 4.3 add / remove — exact bag inverses

The visible multiset of a key K is `visible(K) = max(0, base_mult(K) − tombstone(K)) + overlay(K)`,
with `tombstone(K) ∈ [0, base_mult(K)]`. To make `add-atom`/`remove-atom` exact ±1 inverses:

- **add**: if `tombstone(K) > 0` → `tombstone(K) -= 1` (revive a suppressed base copy) and DO NOT
  also write the overlay; else the normal overlay `+1`.
- **remove**: overlay copies first (`overlay(K) -= 1` via the normal path); only on an overlay miss,
  if `tombstone(K) < base_mult(K)` → `tombstone(K) += 1` (suppress a base copy); else no-op.

*(Deviation from the original prose "unchanged overlay write AND decrement the tombstone" on add:
doing BOTH double-counts — e.g. base=2 then `rm,rm,add` would give 2 instead of 1. The XOR model
above — un-tombstone OR overlay-add, not both — is the corrected, bag-exact semantics, proven by
each branch moving `visible` by exactly ±1 without driving a layer out of range.)* Interposed in
`add_to_space[_shared]` (`untombstone_on_add`) and `remove_from_space[_shared]`
(`remove_overlay_miss_tombstone` → `tombstone_on_remove`), all gated on `has_act_base` and scoped to
literal facts (rules `(= …)` keep their De-Bruijn / RuleIndex path).

### 4.4 compaction — `(compact-space! "name")`

Folds `overlay + (base − tombstones)` into a fresh ACT, atomically replaces the old base, and
re-attaches with a clean overlay/tombstone slate (a SEMANTIC NO-OP on the visible multiset):

1. decode `(base − tombstones)` into owned facts (reads the OLD files, intact);
2. detach, then re-`add` those facts into the overlay (now `self` holds the full content, interned
   with this env's mapping; `add_to_space` re-routes rules/types, rebuilding their registries);
3. `save_space_to_act` to a TEMP name;
4. atomic `rename` the temp `.act`/`.wide.act`/`.sm` over `<name>.*` (the wide sibling is renamed if
   the content has wide facts, else the stale `<name>.wide.act` is removed);
5. invalidate the deserialized-`sm` cache for `name`;
6. clear the overlay (`btm`/`wide_btm`/`variable_atoms`, reset `total_atoms`);
7. RCU re-attach `name` (a fresh `Arc<ActBase>`; the old one was dropped at step 2's detach).

Returns the Σ-multiplicity (total fact count) as a `Long`. The atomic rename means in-flight readers
of the old base (existing mmaps of the unlinked inode) are never corrupted.

### 4.5 Threading through clone / fork / union

- `AtomSpace::new` / `union` (genuine merge): None / empty tombstones / gate off — a genuine merge
  folds all source `btm`s into one in-memory union, so there is no longer a single base; re-attach
  if a base is wanted afterward.
- `AtomSpace::fork` / `make_owned`: `Arc<ActBase>` bump (read-only base sharing) + O(1) `tombstones`
  CoW clone + gate mirror → the owned/forked env keeps tiering with an isolated tombstone copy (env
  clone-isolation invariant: a clone's `attach`/tombstone never leaks back to the source).
- `fork_for_nondeterminism`: Arc-shares the whole `atom_space`, so a nondeterministic branch
  observes the SAME base + tombstone state for free (PeTTa global-atomspace semantics).
- `union` BRANCH-UNION fast paths (`Arc::ptr_eq` / only-one-modified) `Arc::clone` `self.shared` and
  so share the base for free; only the both/all-modified rebuild drops it.

### 4.6 MeTTa surface (T0 special forms)

- `(attach-act-base! "name")` → attach the base into a `clone()`d env; returns the `.act` path
  String (Error if absent / not a valid ACT).
- `(detach-act-base!)` → drop the base + tombstones on a clone; returns Unit (overlay kept).
- `(compact-space! "name")` → compaction on a clone; returns the Σ-multiplicity `Long`.

All three are `is_impure_head` (never memoized) and `can_compile_with_env => false` (T0-only),
mirroring the `save-space!`/`load-space!`/`query-act` ops.

### 4.7 Validation

`src/backend/environment/act_tiered.rs::tests` (19 tests): no-base-fast-path-unchanged; base+overlay
union; overlay-re-add additive multiplicity; tombstone shadowing; remove-then-readd un-hide;
count-based tombstone multiplicity; cross-env attach+query; clone-isolation of attach/tombstone;
fork_for_nondeterminism shares base; detach drops base/keeps overlay; remove-absent no-op;
compaction semantic-no-op + round-trip + multiplicity preservation + wide facts + idempotency.
Plus `tests/act_tiered_surface.rs` (9 tests) through `compile`/`eval`. Full gate after each phase:
`cargo nextest` 4282, mtt-conformance 483/0, PLN-main 0 ❌.

### Performance notes (not gaps)

- Both intra- and cross-run reads stay **trie-pruned** for head-shaped patterns: after the
  `mork_interning` deserialize bucket-hash fix, the join encodes faithfully through a deserialized
  `<name>.sm`, so there is no cross-run scan degradation. (`act_sm_for` is resolved once at attach
  time and cached on `ActBase`.)
- `act_sm_for`'s deserialized `<name>.sm` is cached process-globally (`ACT_SM_CACHE`), invalidated on
  re-save and on compaction's rename — so attach / cross-env query deserialize each `.sm` once.
- `total_atoms` tracks the OVERLAY count only (base facts were never added to this space's overlay
  counter); compaction resets it to the materialized Σ-multiplicity. It is informational, not
  correctness-bearing for the tiered read.
