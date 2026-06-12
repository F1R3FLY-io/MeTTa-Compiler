# Handle-borne variant tags (index mode) — design for the next F1 lever

Status: **IMPLEMENTED + ACCEPTED (experiment #18, observation 220: Robot FANOUT=0 −5.49%, p=5.4e-05, d=0.80; wall green at f3539c65; TWO pre-existing index bugs exposed+fixed by the totality discipline). Honest calibration: the register-negative fraction bought ~5.5%, not the ~20-27% Ir-share projection — cache-hit page-chases pipeline better than instruction counts imply (MLP, measured a second way). v4.1 — R1–R5 complete; R5 NET-SUBTRACTIVE ⇒ CONVERGED. R4 (confirming round) verified the core
lever bit-exactly (recovery arithmetic, oracle soundness, fence
reachability+lock-safety) and was NET-ADDITIVE on two v3 sub-decisions,
folded below: the flags rider is DEFERRED (unimplementable as written),
and the gc_roots exemption is DELETED in favor of fence-first sequencing
(Option A). One final confirming pass (R5) gates pre-registration (#18).**
Data basis: the callgrind-exact post-exp17 profile (`a69dc8de`,
`target/gc-logs/f1_profile_post-exp17/robot_f0.callgrind`).

## v4 amendments (R4 findings 2 & 6 + precision edits)

- **Flags rider DEFERRED (R4-F2):** "carry FLAG_HAS_VARIABLES through the
  same recovery" is impossible — the pack is `tagged >> 4`
  (metta_value.rs:1404), which destroys bits [3:0] BEFORE packing; unlike
  TAG5 the flags are simply absent from the JitValue payload. Recovering
  them needs a PACK-SIDE `inner_ptr()` change (free payload bits [47:37]
  exist) — but `inner_ptr()` output is a hash/identity KEY for five
  subsystems, so that is its own determinism-analyzed future item, NOT a
  rider on this lever. The pre-existing JIT flags-loss bug stays documented
  as-is.
- **Option A adopted; gc_roots exemption DELETED (R4-F6):** sequencing is
  FENCE FIRST (revision 4 lands before any recovery site), after which
  every TAG_PTR payload is `inner_ptr()`-packed and gc_roots.rs:217
  recovers the tag exactly like the other five unpacks. `from_addr` carries
  ONE unconditional `debug_assert!(tag != TAG_UNSET && tag <= 18)`; no
  mark-only constructor, no exemption; revision 8's "tagged == 0 impossible
  for index handles" becomes unconditionally true. (R4 verified the
  no-soundness-hole status quo: the only 0-sentinel reader is fed by
  interpreter handles, never gc_roots output.)
- **Revision 3 precision (R4-F4):** the variant is statically in hand at
  remap-INSERT (the restore fixpoint holds `node: &SerNode` at every
  `remap.insert`, continuation_slice.rs:464-478), not at `resolve()`.
  Implementation: widen the IN-MEMORY remap to `HashMap<u32, (Addr, u8)>`;
  `resolve()` reads the tag from the map. Zero wire change, zero heap
  reads, no lock interaction (the per-node write guard is released before
  resolve_all).
- Optional hardening adopted: the unpack recovery asserts
  `debug_assert!(tag <= 18)` (probabilistic leak detector for any future
  non-`inner_ptr()`-packed payload).

## v3 mandatory revisions (R2 findings 1–6, all local)

1. **JIT unpack tag recovery (R2-F1, MAJOR; R4-verified bit-exact).** The
   tag SURVIVES the pack: `inner_ptr()`'s index arm returns
   `INDEX_KEY_TAG | (tagged >> 4)` (metta_value.rs:1402-1404), placing
   TAG5 at fake-pointer bits [36:32] inside the 48-bit JitValue payload
   (INDEX_KEY_TAG = bit 48, OUTSIDE the payload mask — no collision;
   payload bits [47:37] are zero). Recovery at ALL SIX unpacks =
   `tag = (payload >> 32) & 0x1F`: metta_value.rs:2852 (`from_inner_ptr`),
   jit/types/value.rs:382/402, jit/hybrid/arena.rs:501/520,
   jit/runtime/gc_roots.rs:215→217 (NO exemption — see v4/Option A; the
   fence lands first, so every TAG_PTR payload is `inner_ptr()`-packed).
   (The FLAG_HAS_VARIABLES loss on this path is a PRE-EXISTING separate
   bug — flags do NOT survive the `>>4` pack; deferred, see v4.)
2. **Accessor-side DEBUG oracle (R2-F2, MAJOR).** The inner_ref_index
   tripwire is BLIND to wrong-tag fast negatives (the failure bypasses
   inner_ref_index entirely). The real coupling: in DEBUG builds, on every
   register-negative the accessor ALSO materializes and asserts agreement
   (`debug_assert!(materialized answer == None)`). The mint-side assert
   becomes STRICT (`tag != UNSET`) once revision 1 lands; the 483-fixture
   DEBUG oracle + greenwall then exercise both directions.
3. **E4: derive-at-restore, NO wire change (R2-F4, MAJOR).** SlotRef is
   postcard-serialized (non-self-describing, no version field) and slices
   ship cross-process — adding `tag: u8` silently breaks mixed-version
   peers. Unnecessary anyway: at `resolve` (continuation_slice.rs:129-134)
   the restorer just re-interned the node images and knows each variant
   statically — derive the tag there with zero heap reads.
4. **Index-mode `from_long` overflow fence (R2-F5, MAJOR — pre-existing
   latent bug).** `JitValue::from_long` for |n| > 2^47 ALWAYS
   slab-allocates (jit/types/value.rs:64-68, gc_allocator.rs:1948) even in
   index builds — the TAG_PTR payload is then a real slab pointer whose low
   32 bits an index unpack would misread as an Addr (garbage handle TODAY,
   garbage tag under revision 1). Fence: index-mode overflow mints via
   IndexFactory. Unpack-side recovery TRUSTS ONLY `inner_ptr()`-packed
   payloads.
5. **T2 reworded (R2-F3, MAJOR).** Fail-soft holds ONLY for MISSING
   (UNSET) tags. WRONG-nonzero tags flip accessor answers = CORRECTNESS
   bugs; I1-agreement is the HARD invariant, coupled by revision 2's
   accessor oracle. Also: under a totality gap the perf degradation can be
   SUPER-LINEAR (inner_ptr-derived cache keys become (Addr,tag)-dependent:
   type-inference cycle sets types.rs:1001-1008, alpha-equiv, MORK
   ground_cache, VALUE_HASH_CACHE — key-splitting up to exponential
   re-traversal), not "redundant rebuild." identity_eq has 16 non-test
   call sites (engine.rs ×7, bindings.rs ×9), all rebuild-avoidance.
6. **Placement + cfg (R2-F6).** The accessors have NO mode branch today
   (it lives inside `inner_ref`, which returns `&'static` and cannot
   express the fast-path None). The tag dispatch is a NEW branch per
   accessor: `#[cfg(feature = "index-gc")] if gc_mode_is_index() { ... }`
   — slab production builds byte-identical (compiled out); slab-mode-under-
   index-build (tests) safe via the runtime check.
7. **Scope exclusions (R2-F10).** `as_long`/`as_float` are
   LAZY-transparent (the Direct.metta conf=0.0 phantom fix) — DO NOT apply
   the tag template to them (it would reintroduce that bug). The six
   tagged accessors are Spanned-transparent-only: "TAG_SPANNED falls
   through, else None" is behavior-exact, including Lazy→None.
8. **Bonus hardening (R2-F8).** Strict totality (TAG5 ≥ 1 on every heap
   handle) makes `tagged == 0` impossible for index handles — fixing a
   LIVE latent hole: `CurrentIterRootProvider`'s 0-sentinel
   (current_iter_root.rs:40-44,91) silently drops a legitimate
   Addr(0)-handle from roots today.
9. **Hash-cons (R2-F7, verified safe under totality).** `hash_cons_key`
   hashes raw child words and `intern_ground_sexpr` verifies + EVICTS on
   raw-word mismatch — a totality gap would churn canonical cons cells;
   under totality, deterministic and stable. I2/I3 hold.

## Problem (exact, measured)

`MettaValue::inner_ref_index` is **40.37% of Robot FANOUT=0 program Ir**
(52.1B of 129.0B), reached ~**985M times per run** at ~53 Ir/call — i.e.
shadow **cache hits**: TLS access + two-level page chase + epoch check, per
call. The callers are variant-DISPATCH questions, mostly answered "no":

| caller | calls | edge-incl. share |
|---|---|---|
| `as_atom` | 297.9M | 11.62% |
| `as_sexpr` | 243.8M | 9.57% |
| `collect_variables_generic` | 195.4M | 7.58% |
| `as_atom'2` (Spanned-recursion) | 115.1M | 4.46% |
| `as_error` | 64.3M | 2.49% |
| `find_grounded_arg_indices` | 49.7M | 1.93% |

Slab mode answers the same questions with ONE pointer deref (the pointee's
discriminant). The index's per-question cost is the residual F1 wall gap's
core (post-exp17 ratios 1.235–1.499×).

**Spanned fraction (from the same profile, no new instrumentation needed):**
`as_atom'2` is `as_atom` re-entered through `Spanned(v, _) => v.as_atom()`
⇒ ~115.1M / (297.9M+115.1M) ≈ **28% of atom checks chase one Spanned hop**
on Robot; `as_sexpr'2` 18.6M / (243.8M+18.6M) ≈ **7%**. So 72–93% of the
hot checks are single-level and fully servable by a handle tag; the Spanned
fraction needs ONE fallthrough materialization (status quo cost).

## Design

### Canonical TAG5 mapping (v4.1, R5-F7)

`Node`'s DECLARATION ORDER (index_node.rs:92-117, exactly 18 variants) is
THE canonical mapping: `UNSET = 0`, then `Atom = 1 … Spanned = 18` in
declaration order, emitted by a single `Node::variant_code()` — and the E4
restore derive (`SerNode`, continuation_slice.rs:211-230, which mirrors
`Node` in identical declaration order) MUST share that one table (a
`SerNode::variant_code()` delegating to the same constants). A divergence
trips the I1 DEBUG tripwire on the first materialization of any restored
handle. Index-vacuity note (R5-F2): `from_inner_ptr`'s two non-JIT feeders
(gc_cron.rs:486 counter-sync, gc_allocator.rs:7002 slab GC Phase 2) feed
raw slab-slot pointers but are unreachable in index mode (the slab
collector is index-inert; counter pages receive no index traffic).

### Representation

A `MettaValue` heap handle's tagged word (index mode) currently uses:
- bits [63:48] — NaN-box space, ZERO for heap handles (`is_inline()` false);
- bits [47:36] — **ZERO today (this is the claim to verify in R1)**;
- bits [35:4] — the 32-bit `Addr` (`as_arena_addr` reads `(tagged >> 4) as u32`);
- bits [3:0] — handle flags (`FLAG_HAS_VARIABLES` …).

Allocate bits **[40:36] = `TAG5`**, a 5-bit code for the 18 `Node` variants
(0 = `TAG_UNSET` for backward/edge paths; 1..=18 the variants). Index-ONLY:
slab handles store a real pointer that may use bits up to [47] — the slab
path never sets nor reads TAG5 and stays byte-identical.

### Fast paths (the win)

In each hot accessor (`as_atom`, `as_sexpr`, `as_error`, `as_conjunction`,
`get_head_symbol`'s outer match, `is_*` predicates), after the existing
`is_inline()` early-out and inside the existing `gc_mode_is_index()` branch:

```text
tag = (tagged >> 36) & 0x1F
tag == TAG_ATOM      -> proceed to materialize (payload needed)      [hit]
tag == TAG_SPANNED   -> fallthrough to inner_ref_index (transparent) [28%/7%]
tag == TAG_UNSET     -> fallthrough (conservative; no behavior change)
otherwise            -> return None  ← REGISTER-ONLY, the saved ~53 Ir
```

Payload-carrying hits still materialize (the shadow stays the payload
mechanism — this design does NOT touch materialization, the laundered
&'static contract, or the GC).

### Construction sites (tag-setting)

Single chokepoint preferred: every index handle is minted from an `Addr` at
a small set of sites — the `IndexFactory` intern/alloc returns, and
`MettaValue::from_addr(raw, flags)` (E4 reconstruction). Setting rule:
`tag = variant_of(node)` at mint time, where the node variant is ALREADY
known statically at each factory method (`f.atom(..)` mints `TAG_ATOM` with
no heap read) — `from_addr` gains a mandatory tag parameter. E4 restore
DERIVES the tag at remap-insert (the fixpoint holds the `SerNode` variant
statically; the in-memory remap widens to `HashMap<u32, (Addr, u8)>`) —
zero wire change, zero heap reads (v4/R4-F4; the earlier persisted-field
idea was a silent postcard wire break and is REVERSED).

### Invariants + verification

- **I1 (agreement)**: for every live index handle, `TAG5 ∈ {UNSET,
  variant_of(node_at(addr))}`. Enforced by a DEBUG-build tripwire inside
  `inner_ref_index`: `debug_assert!(tag_unset || tag == node.variant_code())`
  — the conformance DEBUG oracle (483 fixtures + machine-equivalence) then
  exercises it on every materialization, the exp15 permanent-tripwire
  pattern.
- **I2 (Addr-reuse safety)**: a recycled Addr names a NEW node; stale
  handles to swept nodes do not exist by GC soundness (the whole collector's
  proven property) — therefore a handle's tag can never disagree with a
  LIVE node it points to. No epoch logic needed for tags. (Red-team this
  against the per-cell-generation side-index history.)
- **I3 (equality/hashing)**: `tagged`-word equality and `hash_value` —
  same node ⇒ same Addr ⇒ same mint path ⇒ same TAG5, so tag bits never
  split identical values. Hazard: any path comparing handles minted BEFORE
  and AFTER this change within one process (none — no persistence of raw
  tagged words across runs except E4 slices, which derive tags at restore
  from the re-interned node variants, deterministically).
- **I4 (Addr extraction)**: `as_arena_addr` truncates to u32 after `>>4` —
  unaffected by bits [40:36]. Audit every OTHER reader of the raw word in
  index mode (grep `tagged >>`, `& PTR_MASK`, transmutes) for masks that
  would now see nonzero [40:36].

### Formal/oracle obligations (per the house method)

- The tag is a pure CACHE of the node variant — a small Rocq note
  (`HandleTagAgreement.v`) stating I1+I2 compositionally over the existing
  collector-soundness theorem, plus the DEBUG tripwire as the runtime
  coupling. No new TLA+ (no concurrency: tags are immutable after mint).

### Projected effect (to be locked in #18 only after red-team)

Register-only negatives for ~70–90% of ~985M calls ⇒ ~30–45B Ir removed ≈
23–35% of the 129B program ⇒ Robot ~10.6s → ~7.5–8.5s ≈ **ratio
1.50× → ~1.06–1.20×** (slab side untouched this time — index-only change).
Criterion: the standard Welch one-tailed α=0.05, d≥0.5, n=51 interleaved.

## v2 refinements (from R1/R3 + the raw-word reader audit)

- **T1 — TOTALITY (load-bearing):** every index mint path MUST set
  `TAG5 ≠ UNSET`. `from_addr(raw, flags)` gains a mandatory `tag` parameter;
  E4 restore derives tags at remap-insert (NO wire field — see v4; the
  `SlotRef` postcard format is untouched). Rationale:
  `PartialEq::eq` (metta_value.rs:2681) and the heap path (2698,
  `tagged & PTR_MASK` — PTR_MASK KEEPS bits [40:36]) compare raw words, and
  `identity_eq` (2841) is a raw-word identity — a mixed tagged/untagged pair
  for the same node would miss those fast paths. Under totality they never
  mix.
- **T2 — FAIL-SOFT (scope NARROWED by v3 revision 5: the UNSET direction
  only — wrong-nonzero tags are CORRECTNESS bugs; I1 is the hard
  invariant):** even under a MISSING-tag totality bug, no raw-word reader
  is correctness-relevant:
  `PartialEq` falls through to structural `inner() == inner()` (2704);
  `identity_eq`'s 8 call sites are all trampoline REBUILD-AVOIDANCE checks
  (engine.rs:305-372, bindings.rs:1126) where a false-negative causes a
  redundant rebuild, not a wrong answer; `Hash` is structure-keyed (never
  reads raw bits). A tagging bug degrades PERFORMANCE only — and the I1
  DEBUG tripwire (483-fixture oracle + the greenwall) catches it
  empirically.
- Spanned NESTING is allowed by design (`Spanned(Spanned(v, binding_span),
  template_span)`, index_node.rs:718) — multi-hop chases exist; the 28%/7%
  estimates are single-hop lower bounds. Cost-model note only (the tag
  fast path is unaffected; Spanned always falls through).
- Tagged-accessor set (R3): `as_atom`, `as_sexpr`, `as_error`,
  `as_conjunction`, `as_quoted_ref`, `get_head_symbol`'s outer match, plus
  the `is_*` predicates that route through `inner_ref`. All six hot callers
  (985M calls) dispatch exclusively through this set — verified; no
  >10M-call direct `inner_ref` users exist in the eval hot loop.

## Red-team ledger

- ✅ R1 (2026-06-12, source-verified): bits [47:36] = 0 on EVERY index mint
  path (`from_addr` 576: u32 `<<4`; factory sites; inline NB tags live at
  [63:48]); `is_inline` tests [63:48] only; E4 slices carry `(raw: u32,
  flags: u8)` via `addr_flags() & 0xF`; hash is structure-keyed. NO
  falsifications. PLUS the finding that raw-word equality/identity comparers
  exist → folded into T1/T2 above.
- ✅ R3 (2026-06-12, source-verified): all six hot callers dispatch via the
  `as_*` family (collect_variables_generic uses as_atom/as_sexpr/
  as_conjunction/as_quoted_ref/as_error exclusively — bindings.rs:104-130);
  find_grounded uses as_sexpr/as_atom (grounded.rs:181-210); `view()` is not
  on the hot path; no coverage reductions.
- ✅ R2 (2026-06-12): exhaustive mint-site census (5 classes, all
  file:line-enumerated); verdict CONVERGED with 4 mandatory revisions —
  folded as v3.
- ✅ R4 (2026-06-12, confirming): core lever verified bit-exactly (recovery
  arithmetic incl. INDEX_KEY_TAG non-collision; oracle soundness by
  status-quo equivalence; fence reachability + lock-safety by the JIT's
  existing factory-mint precedent; six-site unpack list complete).
  NET-ADDITIVE on two v3 sub-decisions (flags rider unimplementable;
  exemption/assert contradiction) — folded as v4 (rider deferred; Option A
  fence-first, exemption deleted).
- ✅ R5 (2026-06-12, final confirming): every v4 amendment survived direct
  attack (fence census closed — TAG_ERROR never minted; assert has a tag
  source at every production call site; widened remap wire-invisible by
  type). NET-SUBTRACTIVE ⇒ CONVERGED at v4.1 (3 editorial touch-ups) ⇒
  pre-register #18.
