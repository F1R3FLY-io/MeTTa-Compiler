# Allocation Optimization Benchmark Results

## Environment

- Platform: macOS (Darwin 25.3.0), Apple Silicon
- Rust: release profile with `target-cpu=native`
- Benchmark tool: `hyperfine --warmup 3 --runs 10`

## Baseline (pre-optimization, sequential isolated runs)

| Benchmark | Mean +/- σ |
|-----------|------------|
| PLN Robot | 1.495s +/- 0.049s |
| mmverify  | 39.6ms +/- 2.9ms |

## Cumulative Results

### After Phases 1+2 (alloc_slice_copy + skip-Spanned)

| Benchmark | Mean +/- σ | Delta |
|-----------|------------|-------|
| PLN Robot | 1.438s +/- 0.017s | -3.8% |
| mmverify  | 34.1ms +/- 1.2ms | -13.9% |

### After Phases 1+2+3 (+ reuse sexpr in processing)

| Benchmark | Mean +/- σ | Delta |
|-----------|------------|-------|
| PLN Robot | 1.430s +/- 0.015s | -4.3% |
| mmverify  | 35.2ms +/- 1.4ms | -11.1% |

### After All Phases (1-5) + Lazy ground_cache GC Validation

| Benchmark | Mean +/- σ | Delta | Notes |
|-----------|------------|-------|-------|
| PLN Robot | (pending re-run) | | Initial run concurrent with stress test |
| mmverify  | 38.9ms +/- 2.3ms | -1.8% | |

## Phases Summary

| Phase | File | Change |
|-------|------|--------|
| 1 | gc_allocator.rs | `alloc_slice_from_iter` -> `alloc_slice_copy` |
| 2 | generic_step.rs, bindings_generic.rs | Skip Spanned wrapping when span present |
| 3 | processing/generic.rs | Reuse sexpr, eliminate clone + redundant alloc |
| 4 | parser/mod.rs | SmallVec in parse_list |
| 5 | compile.rs | `with_capacity` hints |
| Bug fix | mork_convert.rs, gc_allocator.rs | Lazy per-entry ground_cache GC validation |

## Bug Fix: Stale MORK ground_cache after GC Slot Reuse

**Problem**: ~1% intermittent crash in PLN Robot caused by MORK `ground_cache`
replaying stale serialized byte fragments after GC frees and reuses slab slots.
Panic at `rule_management.rs:1617` with `reserved 64` or `0xff`.

**Root cause**: `ground_cache` keyed by `inner_ptr as usize`. After GC sweep
frees slots and addresses are reused for different values, cache returns stale
serialized bytes for the new value at that address.

**Fix**: Replaced "clear entire cache on GC epoch change" with per-entry lazy
validation using the per-slot allocation epoch stored in `ValuePage::epochs[idx]`.
Each cache entry stores the slot's `alloc_epoch` at insertion time. After a GC
sweep epoch change, lookups validate the stored epoch against the slot's current
epoch. Only stale entries are evicted; valid entries survive across GC cycles.

**Verification**: 1000 sequential PLN Robot runs with 0 crashes (previously ~1%).

## Stress Test Results

- 1000 sequential runs of PLN Robot: 0 crashes
- Debug build PLN Robot run: passed (exercises MORK integrity check)
