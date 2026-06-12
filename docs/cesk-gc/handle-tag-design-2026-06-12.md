# Handle-borne variant tags (index mode) — design for the next F1 lever

Status: **v3 — R1/R2/R3 complete, R2 verdict CONVERGED with 4 mandatory
revisions (folded below); one confirming round (R4) before pre-registration
(experiment #18).** Data basis: the callgrind-exact post-exp17 profile
(`a69dc8de`, `target/gc-logs/f1_profile_post-exp17/robot_f0.callgrind`).

## v3 mandatory revisions (R2 findings 1–6, all local)

1. **JIT unpack tag recovery (R2-F1, MAJOR).** The tag SURVIVES the pack:
   `inner_ptr()`'s index arm returns `INDEX_KEY_TAG | (tagged >> 4)`
   (metta_value.rs:1402-1404), placing TAG5 at fake-pointer bits [36:32]
   inside the 48-bit JitValue payload. Recovery at ALL SIX unpacks =
   `tag = (payload >> 32) & 0x1F`: metta_value.rs:2852 (`from_inner_ptr`),
   jit/types/value.rs:382/402, jit/hybrid/arena.rs:501/520,
   jit/runtime/gc_roots.rs:215. EXEMPTION: gc_roots.rs:217 may mint UNSET
   with a documented "mark-only, never materialized" note (marking uses
   `as_arena_addr` exclusively; a heap read there risks lock-order issues).
   Precedent fixed alongside: this path already loses FLAG_HAS_VARIABLES
   (the same metadata-loss class) — carry flags through the same recovery.
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
no heap read) — `from_addr` gains a tag parameter persisted in the E4 slice
beside `(raw, flags)` (slices re-intern identical content ⇒ identical
variant; persisting beats re-deriving, which would need a heap read).

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
  tagged words across runs except E4 slices, which carry their own tag
  field explicitly and reconstruct deterministically).
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
  the E4 `SlotRef::Heap { raw, flags }` gains a `tag: u8` field (slices
  reconstruct deterministically — same content ⇒ same variant). Rationale:
  `PartialEq::eq` (metta_value.rs:2681) and the heap path (2698,
  `tagged & PTR_MASK` — PTR_MASK KEEPS bits [40:36]) compare raw words, and
  `identity_eq` (2841) is a raw-word identity — a mixed tagged/untagged pair
  for the same node would miss those fast paths. Under totality they never
  mix.
- **T2 — FAIL-SOFT (verified, the design's strongest property):** even
  under a totality BUG, no raw-word reader is correctness-relevant:
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
- R2 (pending): adversarial pass on the v2 design (T1 totality enumeration:
  EVERY mint site listed and tag-assigned; hash_cons_key interplay;
  Lazy/Quoted wrapper mint paths; JIT `from_inner_ptr` index packing
  (metta_value.rs:2846+) — does the JIT mint raw handles that bypass
  from_addr?; `peel_span` paths; the inline-Long NB_TAG_LONG slab-alloc
  path).
