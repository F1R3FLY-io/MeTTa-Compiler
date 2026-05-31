# Phase B4 — JIT-on-index re-enablement (design; Plan-agent, pending implementation-review)

Designed by a Plan agent against HEAD `efb7d35` (B2 done; B3 mark-ordering gating). **This is the design;
I (main) will adversarially review each edit against source during implementation** (as for B2/B3 — the
Plan agent's literal claims/line-numbers are verified-then-trusted, not trusted blindly).

## Problem
Under `--features index-gc` the T2/T3 JIT is **hard-disabled** (CAVEAT 2) → index runs the VM without JIT
while slab runs with JIT → index PLN ~2.2× slower (Robot 22s vs 10s, FlyingRaven 49s vs 19s). B4 re-enables
JIT under index-gc. **It's 2 commits, not a rewrite** — Inc 2b already proved the JIT generates NO embedded
value pointers (Cranelift emits `iconst(idx)+call`; all decode is Rust-side) and the NaN-box `TAG_PTR`
payload packs an `Addr` trivially (`inner_ptr()` under index = `INDEX_KEY_TAG(1<<48) | addr.raw()`; the
`& PAYLOAD_MASK` (48-bit) drops bit-48 → exactly `addr.raw()`; a u32 index fits 48 bits).

## Root-cause (the precise incompatibilities, verified)
- **Pack: already correct** (`helpers.rs::metta_to_jit`, `value.rs` ctors — `& PAYLOAD_MASK` masks the tag).
- **Unpack: 2 of 4 decoders already index-aware** (`value.rs::to_metta` ✓, `metta_value.rs::from_inner_ptr` ✓).
- **Unpack: 2 decoders BUGGY (deref payload as a slab `*const MettaValueInner`):**
  - `bytecode/jit/hybrid/arena.rs::jit_to_value` (TAG_PTR ~462-464, TAG_ERROR ~471-473) — the **result decoder**
    for every JIT execution. Under index the payload is `addr.raw()` (e.g. `0x4001`) → `&*ptr` reads garbage
    → silent-wrong/segfault. **The headline correctness bug.**
  - `bytecode/jit/runtime/gc_roots.rs::collect_jit_value_into` (~150-164) — the **root walker**; same
    deref-as-slab-ptr → corrupts the root set + UAF.
  - (`helpers.rs::jit_to_value_generic` ~156-160: `V::from_inner_ptr` self-corrects for `V=MettaValue`, but
    its `!ptr.is_null()` debug-assert fires spuriously on `Addr(0)` → mode-gate the assert.)
- **The JIT register file is NOT a structural GC root** — there is no `VmLeaf::Jit` in the K-spine
  (`cesk/k_spine.rs` notes "JIT gated off under index-gc"). When a JIT runtime callback re-enters
  `eval_trampoline` (`call_support.rs:204`) and the index midloop collector fires (`eval_loop.rs:3788`), the
  `Addr`s in the executor's `jit_stack`/`jit_results`/`choice_points`/`binding_frames`/… are invisible → UAF.
  **The load-bearing B4 ASAN crux.** (The slab-era guard at `call_support.rs:161-171` —
  `collect_jit_roots_into → worker_cooperative_safepoint` on the slab worker-request rendezvous — is a
  discovery-style side-channel, slab-shaped, and must NOT be the index mechanism: it would re-introduce
  exactly what Phase A deleted. Cfg-wall it to slab.)

## The A4 K-leaf contract (how the JIT becomes a structural root)
Mirror the VM exactly. The VM pushes `VmLeafGuard::push(VmLeaf::Vm { vm: *const … })` (RAII, LIFO, raw ptr
read live at collection — never clone-at-push) in `with_vm_roots_frame`; `collect_k_spine` (k_spine.rs:144)
reads it via `(*vm).collect_roots_into(out)`; `collect_machine_roots` (called by the midloop + quiescence
collectors) walks it. B4 adds **`VmLeaf::Jit { ctx: *const JitContext }`** + a `collect_k_spine` arm calling
the (index-fixed) `collect_jit_roots_into(&*ctx, out)` — which already walks every value-bearing JIT field.

## Design — 2 commits

### B4.1 — index-aware JIT value decode (latent correctness; JIT still gated off)
Gate each on `gc_mode_is_index()`; **slab arm byte-identical** (keeps `from_inner(&*ptr)` verbatim):
- `jit/hybrid/arena.rs::jit_to_value` TAG_PTR+TAG_ERROR → index arm `MettaValue::from_addr(Addr::from_raw((jit_val & PAYLOAD_MASK) as u32), 0)`; mode-gate the null-assert.
- `jit/runtime/gc_roots.rs::collect_jit_value_into` → same index reconstruction (do NOT deref).
- `jit/runtime/helpers.rs::jit_to_value_generic` → mode-gate the null-assert (already self-corrects).
- Pack side: no change (verify with an index round-trip unit test, mirroring `helpers.rs::jit_value_roundtrips_in_index_mode`).
Gate: full wall (both builds unchanged — JIT still off) + new index round-trip tests. Revertible.

### B4.2 — JIT K-leaf + ungate
- `cesk/k_spine.rs`: add `VmLeaf::Jit { ctx: *const JitContext }` + the `collect_k_spine` arm → `collect_jit_roots_into`. Document SAFETY identically to `VmLeaf::Vm` (ptr read live; data outlives guard; LIFO).
- `jit/hybrid/arena.rs`: push a `VmLeafGuard` (index-gated, `None` in slab) around `native_fn(&mut ctx)` in BOTH `execute_jit_arena_with_env` (~313) and `execute_jit_arena_direct` (~141) — `&ctx` taken after field setup, held across result-collection.
- `jit/runtime/call_support.rs`: cfg-wall the slab discovery block (155-171) to `not(index-gc)` (the index path is unambiguously the structural K-leaf).
- `tiered_cache.rs`: ungate ALL 8 sites — `1297`/`1491` (trigger gates, the primary lever) + `1698`/`1703` (get_best_tier report) + `2122`/`2133`/`2201`/`2216` (dispatch arms). (`eval/mod.rs:447/476` forced+auto path needs no edit — becomes live once `jitN_status` can reach Ready.)
Gate: the FULL gate below.

## Gate (B4.2)
- **Slab byte-identical:** `cargo nextest run --release` green (JIT-under-slab untouched — all index arms gated).
- **Byte-identical conformance WITH JIT on under index:** `--features index-gc` + `FANOUT_DEPTH=0 MIN_BYTES=131072` + `mtt-conformance --strict` → 483/0 (~840 cycles), index nextest green. Catches the `jit_to_value` decode bug (a wrong `Addr`→value diverges from the VM immediately — the VM is the oracle).
- **Machine-equivalence oracle 0-panics** (debug index-gc): the new `VmLeaf::Jit` must keep the structural root multiset ⊇ the discovered set — mechanically discharges the K-leaf wiring.
- **ASAN @ FANOUT=0 — the load-bearing crux:** force a collection while JIT code holds `Addr`s across the re-entrant `eval_trampoline` (a hot expr with a sub-expr that re-enters eval, `MIN_BYTES=131072` so the midloop fires inside the JIT callback). Without `VmLeaf::Jit` → guaranteed UAF; with it → clean. Capped `-p MemoryMax=32G -p MemorySwapMax=0 -j4`, FOREGROUND.
- **PLN budgets (B4's whole point):** re-measure index+JIT Robot/FlyingRaven (index-vs-index, JIT-off→JIT-on, to isolate the JIT win). Target: drop from 22s/49s toward slab's 10s/19s — land inside the per-rung budgets (Robot ≤12s, FlyingRaven ≤25s). This is the success criterion + the precondition for Phase F's honest Welch.
- **20-run determinism** (FANOUT=0).

## Risks (B4-specific)
- JIT decodes `Addr` as slab ptr → segfault/garbage → B4.1 round-trip test + conformance.
- JIT register file invisible → UAF → **ASAN@FANOUT=0 with collection forced mid-JIT-callback** (the crux).
- Clone-at-push staleness → hold `*const JitContext` read live (mirror VM), NOT snapshot.
- Re-introducing a discovery side-channel → cfg-wall `call_support.rs:161-171` to slab; the oracle catches it.
- Slab regression → every index arm `gc_mode_is_index()`/cfg-gated; slab nextest byte-identical.
- Incomplete decoder set (a 4th slab-deref path) → B4.0 `grep from_inner` audit across `jit/` + conformance.

## Critical files
- `tiered_cache.rs` — 8 ungate sites (1297/1491/1698/1703/2122/2133/2201/2216).
- `jit/hybrid/arena.rs` — `jit_to_value` decode fix + `VmLeafGuard` around `native_fn` (×2 entry points).
- `jit/runtime/gc_roots.rs` — `collect_jit_value_into` index decode fix (becomes the structural JIT-leaf reader).
- `cesk/k_spine.rs` — `VmLeaf::Jit` + the `collect_k_spine` arm.
- `jit/runtime/call_support.rs` — cfg-wall the slab discovery block (155-171) to `not(index-gc)`.
