# F3 Default Store Contract

Date: 2026-06-14

## Contract

F3 changes the Cargo default from the old slab store to the CESK index store.
The contract is compile-time exclusive:

- `cargo build --release --bin mettatron` builds the default index store.
- `cargo build --release --no-default-features --features legacy-slab-gc --bin mettatron` builds the legacy slab opt-out.
- `index-gc` and `legacy-slab-gc` are mutually exclusive.
- A build with neither store feature is rejected at compile time.
- `--gc` and `MTT_GC` assert/report the compiled store; they do not switch stores at runtime.

## Proof Driver

`formal/rocq/gc/DefaultStoreSelection.v` proves the feature-selection policy used
by F3. `scripts/verify_cesk_gc_source_coupling.sh` binds that theorem to source:

- `Cargo.toml` defaults include `index-gc`.
- `Cargo.toml` exposes `legacy-slab-gc` only as an explicit opt-out feature set.
- `src/lib.rs` rejects both-store and no-store selections.
- live gates build default-index arms without `--features index-gc` and build slab
  comparison arms with `--no-default-features --features legacy-slab-gc`.
- CLI/conformance `--gc` paths call `assert_gc_request` before evaluation.

## Soak Gate

Run:

```bash
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% --quiet \
  bash scripts/f3_default_store_soak.sh <label>
```

The soak rebuilds fresh binaries before probing them. It checks:

- default-env CLI stdin with `--gc index` reports `GC store = index` and evaluates `!(+ 1 2)` to `[3]`.
- default-env REPL with `--gc index` reports `GC store = index`, evaluates the same expression to `[3]`, and exits through `quit`.
- a slab assertion against the default-index binary fails before evaluation and points to
  ``--no-default-features --features legacy-slab-gc``.
- the legacy slab opt-out binary accepts `--gc slab` and evaluates the same workload.
- the default-index release binary is restored afterward.

## Evidence Snapshot

Current F3 evidence at this point:

- `0aaede8e` aligned live gates with the proved default-index contract.
- `scripts/verify_cesk_gc_formal.sh` passed under capped `systemd-run`, covering
  proof hygiene, Lean, Rocq, TLC, and source coupling.
- `scripts/ab_gc_diff.sh --no-nextest` passed with 735 legacy-slab/default-index
  conformance fixture outcomes identical.
- the soak gate above is the REPL/default-env evidence required before claiming F3 done.
