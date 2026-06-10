# Finding 2 — confine the laundered `&'static str` via PERMANENT ATOM INTERNING (Option A)

> Read-only exploration (Plan agent, 2026-06-10). Correct-by-construction, ZERO perf hit (a WIN). Replaces the rejected `as_atom`→`&self` tie (26 errors + clone-to-owned in hot index paths).

## Root cause (confirmed)
`as_atom(&self) -> Option<&'static str>` (metta_value_trait.rs:168; impls metta_value.rs:1585/:2897) laund­ers a `&'static`. In SLAB mode atom bytes are `alloc_str`'d into the leaked slab (gc_allocator.rs:1999) → HONEST. In INDEX mode atom bytes are a `Box<str>` in a GC'd `SideColumn<str>` (index_heap.rs `intern_bytes_in :687`; `SideColumn::free :3164` drops the Box at sweep) → the `&'static` is a LIE → Finding-1 UAF. Decode laund­ers via `launder` (index_heap.rs:132 transmute) at `str_slice`/`view_at :839`/`materialize_inner :865`. `INNER_SHADOW` (metta_value.rs:881) is cleared at sweep, so a copied-out `&'static` dangles.

## Decisive infra ALREADY present
- `src/backend/symbol.rs:39` — `static INTERNER: OnceLock<ThreadedRodeo>` (perpetual, thread-safe, process-lifetime). `Symbol::as_str -> &'static str` (:66) is ALREADY honestly `'static`. Ships by default (Cargo.toml:267 includes `symbol-interning`). Barely wired in.
- Freshening ALREADY perpetually leaks atom names into the slab in BOTH modes (freshening.rs:246 `alloc_str`), bounded by `FRESH_NAME_CACHE` (cap 1024 :218) — so "atoms never freed" is an ALREADY-ACCEPTED property.
- Precedent: adaptive_indexing.rs:145 keys on a u64 hash "to avoid &'static str lifetime issues".

## Plan (Option A; A1 = perf-optimal: store the interned ref in the Node)
1. **symbol.rs**: add `pub fn intern_static(s: &str) -> &'static str { interner().resolve(&interner().get_or_intern(s)) }` (+ a `not(feature="symbol-interning")` fallback to `global_allocator().alloc_str`).
2. **Index atom alloc** (index_heap.rs `alloc_atom :490`, `IndexFactory::atom :1594`): intern via `intern_static` and store the honest `&'static` in the Node — `Node::Atom(&'static str)` (index_node.rs; was `Node::Atom(ByteRef)`, 8B→16B, still ≤32B `Node` budget) — NO side-column box for atoms. (String keeps `ByteRef`/side-column.) Decode arms `str_slice`/`view_at :839`/`materialize_inner :865` return the stored `&'static` directly — DELETE the `launder` for atoms (removes the `unsafe`). emit_node (E4 capture) atom arm: `to_string()` the stored `&'static` (works unchanged). Atom has no children → child_addrs_for_mark unaffected; Atom no longer has a side-box → sweep/side-reclaim for atoms gone (FEWER side reclaims ⇒ #273 still holds).
   - MINIMAL fallback A2 (if A1's Node change is too broad): keep `Node::Atom(ByteRef)` + the side-box, but make the decode arms `intern_static(str_slice(addr))` (honest interner ptr, drop launder). Downside: per-decode intern cost + double storage. PREFER A1.
3. **Slab atom** (gc_allocator.rs `GcFactory::atom :8000`): OPTIONAL — `intern_static` for dedup. To MINIMIZE A/B risk, can leave slab on `alloc_str` (already honest); only the INDEX path needs the fix. Decide via the A/B gate.
4. `as_atom` SIGNATURE UNCHANGED (`-> Option<&'static str>`, now honest both modes). Update docs (metta_value_trait.rs:167, metta_value.rs:1583): "interned in the global perpetual rodeo; honest in both modes." ZERO call-site churn (682 sites + 26 sinks compile unchanged, no clones).
5. Freshening (freshening.rs:231 `intern_fresh_name`): switch `alloc_str` → `intern_static` (share the dedup table; ≤ today's leak).
6. (DEFER) Option B: `MettaValueInner::Atom`/`Node::Atom` → `Spur` for u32-eq — extra speed, bigger churn; layer later.

## Formal (mirror TrackedVarSideRetention.v)
`formal/rocq/gc/InternedAtomNeverFreed.v` — invert "rooted⇒retained" to "interned⇒not-in-collectable-domain": `Variable Interned`, hypothesis `intern_outside_arena: Interned b -> ~ exists a, SideBoxOf a b` (coupled to step 2: atoms not in any SideColumn), MAIN `interned_atom_bytes_never_released: Interned b -> ~ Released b` (trivial — Released only applies to side/segment cells), non-vacuity `pre_fix_sidebox_atom_releasable` (the old Node::Atom(ByteRef)→SideBox→Released path). Admit+axiom-free. Wire into verify_cesk_gc_formal.sh + source-coupling pins (alloc interns / no side-box for atoms / symbol.rs perpetual OnceLock). Lets us DELETE the atom `launder` `unsafe`.

## Verification ladder
1. **A/B byte-identity BOTH modes — `scripts/ab_gc_diff.sh`** (GATING): interning changes the atom POINTER; audit for any test asserting an atom ADDRESS (`std::ptr::eq`/`inner_ptr` near Atom) vs value/equality. Content + equality preserved. `cargo test` both modes.
2. ASAN both modes (`scripts/a5_asan_both.sh`, the FANOUT rendezvous path that hit Finding 1) → 0 UAF (the freed-side-box read is now impossible).
3. greenwall `scripts/a5_greenwall.sh --with-oracle`: 483/0 both modes, warnings 49/49.
4. Perf (`scripts/c_ab_bench.sh`): expect fewer allocs + lower committed growth + faster compares ⇒ ≥ parity / a win.
5. Rocq InternedAtomNeverFreed.v admit-free; FORMAL_RC=0.

## Critical files
symbol.rs (:39/:66 + new intern_static); index_heap.rs (launder :132, intern_bytes_in :687, str_slice :773, view_at :839, materialize_inner :865, alloc_atom :490, IndexFactory::atom :1594, SideColumn::free :3152); index_node.rs (Node::Atom variant); metta_value.rs (MettaValueInner::Atom :656, as_atom :1585/:2897, INNER_SHADOW :881); gc_allocator.rs (GcFactory::atom :8000, alloc_str :1999); freshening.rs (:231/:246); formal/rocq/gc/TrackedVarSideRetention.v (mirror).
