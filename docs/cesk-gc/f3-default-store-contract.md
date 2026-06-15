# F3 Default Store Contract

Date: 2026-06-14

## Contract

F3 changes the Cargo default from the old slab store to the CESK index store.
The contract is compile-time exclusive:

- `cargo build --release --bin mettatron` builds the default index store.
- `index-gc` is mandatory; the legacy slab store was decommissioned (F4 R2) and there is no alternate store feature.
- A build that drops `index-gc` (e.g. `--no-default-features`) is rejected at compile time.
- `--gc` and `MTT_GC` assert/report the compiled store; they do not switch stores at runtime.

## Proof Driver

`formal/rocq/gc/DefaultStoreSelection.v` proves the feature-selection policy used
by F3. `scripts/verify_cesk_gc_source_coupling.sh` binds that theorem to source:

- `Cargo.toml` defaults include `index-gc`.
- `Cargo.toml` no longer exposes a legacy slab feature; `index-gc` is the only store.
- `src/lib.rs` rejects a build that drops `index-gc`.
- live gates build the default-index arm only; the legacy slab comparison arm was removed at F4 R2.
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
- a slab assertion against the default-index binary fails before evaluation, reporting the
  slab store is decommissioned.

## Evidence Snapshot

Current F3 evidence at this point:

- `0aaede8e` aligned live gates with the proved default-index contract.
- `scripts/verify_cesk_gc_formal.sh` passed under capped `systemd-run`, covering
  proof hygiene, Lean, Rocq, TLC, and source coupling.
- `scripts/ab_gc_diff.sh --no-nextest` (since retired at F4 R2) had passed with 735
  legacy-slab/default-index conformance fixture outcomes identical, evidencing the F1 A/B.
- the soak gate above is the REPL/default-env evidence required before claiming F3 done.
