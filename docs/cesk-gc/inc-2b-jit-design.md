# Inc 2b — JIT/bytecode tier under index mode (implementation design)

> GC migration, after Inc 2a (CRUX) complete. Source-verified Plan-agent design,
> 2026-05-28. Slab byte-identical (every index arm behind `gc_mode_is_index()`).

## Verified ground truth
- **No `JitContext.value_mode` field exists** (context.rs:154-392; doc-comments at :344/:352
  reference a field never added). Arena-vs-heap = whether `ctx.arena` is null. So "extend
  value_mode" is rejected.
- **VM tier (T1) is already index-correct**: `bytecode/mod.rs:1066` runs the generic VM over
  `V: MettaValueTrait` (no JitValue/from_inner); `vm/mod.rs:1024 collect_roots_into` only clones
  `V` handles. **Only the JIT (T2/T3) FFI boundary is index-broken.**
- **Generated Cranelift code embeds no value pointer** (handlers/values.rs:84-131 emits
  `iconst(idx)` + `call` → opaque u64; all decode is Rust-side). Claim verified.
- `inner_ptr()` in index mode = `INDEX_KEY_TAG(1<<48) | (tagged>>4)` — a non-deref key. JIT payload
  budget is 48 bits (`PAYLOAD_MASK=0xFFFF_FFFFFFFF`); `from_inner_ptr` `debug_assert`s bit-48 clear
  → the INDEX_KEY_TAG'd key would trip it, but the **bare 32-bit Addr fits trivially**.
- Shadow-cache (`INNER_SHADOW`) is NOT cleared at the safepoint in 2b (Inc 6 wires that); no JIT path
  holds `&MettaValueInner` across a safepoint (decode → Copy handle immediately). No UAF in 2b.
- **Tests**: `tests/jit_comparison.rs` and `M18-jit-fallback` DO NOT EXIST. Real: `tests/tiered_execution.rs`,
  `tests/cross_tier_bisim_proptest.rs`, `jit_binding_hash`, `jit_long_range`, `jit_choicepoint_overflow`,
  `act_tiered_surface`. Conformance modules are `ln-pt`/`ln-he` filtered by `M11-`.

## Structure decision: (iii)-primary + (ii)-enabling
Disable T2/T3 JIT under `gc_mode_is_index()` → fall back to the verified-correct VM(T1)/T0
(explicit + tested fallback, plan-sanctioned, NOT a stub). ALSO make the Rust-side pack/unpack
helpers mode-aware (correct if reached, re-enablable, no latent UAF). Both gated → Slab unchanged.

## Ordered steps
- **2b-1** mode-aware JIT *pack* (`inner_ptr()`→Addr bits). `helpers.rs:56` metta_to_jit, `:103`
  value_to_jit_generic, `:73/:81` error helpers. New `jit_payload_bits(val)`: `as_arena_addr()→
  Some(addr.raw() as u64)` (index) else `!gc_mode_is_index()→inner_ptr()&PAYLOAD_MASK` (slab,
  unchanged) else None(inline). Test: index pack → payload==addr.raw(), bit48 clear.
- **2b-2** mode-aware JIT *unpack* (`from_inner(&*ptr)`→`from_addr`). `helpers.rs:156`,
  `value.rs:373-402` (TAG_PTR+TAG_ERROR), `arena.rs:462-479` (×2), `sexpr_ops.rs:74/126/259/297/343/399`.
  New `jit_payload_to_value(bits)`: index→`MettaValue::from_addr(Addr::from_raw((bits&PAYLOAD_MASK) as u32),0)`;
  slab→`from_inner(&*ptr)` (unchanged, flags=0 matches from_inner). Mode-gate the is_null guards
  (Addr(0) is valid). DO NOT touch TAG_ATOM/TAG_VAR arms (interned `*const String`, mode-independent).
  Test: round-trip `jit_to_value(metta_to_jit(v))==v` for index-built values.
- **2b-3** `gc_roots.rs:149-164 collect_jit_value_into` — reuse 2b-2 unpacker (reconstruct handle, not
  deref). Inc 6 obligation: the live index mark must walk these handles' Addrs (no live mark in 2b).
  Test: index SExpr in value_stack → collected handle's as_arena_addr()==original.
- **2b-4** gate JIT dispatch off in index mode → VM fallback. `tiered_cache.rs:2164-2189` add
  `!gc_mode_is_index() &&` to both jit2/jit1 Ready arms (bytecode arm :2190 runs, index-correct).
  Companion: `maybe_trigger_jit1/2` (:1290+) early-return if gc_mode_is_index() (no Cranelift work).
  Leave `increment_and_get_hash` AS-IS (stable key; gate makes its slab-miss irrelevant). Test
  `test_index_mode_falls_back_to_bytecode_tier`: hot expr in index mode → jit1_status==NotStarted +
  correct results; Slab still promotes.
- **2b-5** audit+test shadow-cache lifetime. No JIT path holds `&MettaValueInner` across safepoint
  (all decodes → Copy handle). Test: materialize→pack/unpack→clear_inner_shadow()→handle still
  resolves via view()/as_arena_addr() (handles mode-stable; only the box cleared).

## Exit gate
Slab: nextest 4311/4311, mtt-conformance 483, M11-pt 221, M11-he 40. Plus: `tests/tiered_execution`
(incl. new fallback test), `cross_tier_bisim_proptest` (+ index arm), jit_binding_hash/long_range/
choicepoint_overflow, act_tiered_surface, jit::runtime::{helpers,gc_roots} unit round-trips. (No
jit_comparison.rs — substitute cross_tier_bisim_proptest.)

## Inc 6 carry-forward (not dropped)
Wire clear_inner_shadow() into the sweep safepoint; consume 2b-3's reconstructed JIT roots in the live
mark (+ JIT-root-coverage test mirroring the VM-choice-point oracle); re-audit JIT value-field lifetime
if new value-bearing JitContext fields are added; optional JIT-on-index re-enablement (flip 2b-4 gates,
add index jit_comparison A/B) is unblocked but deferred.
