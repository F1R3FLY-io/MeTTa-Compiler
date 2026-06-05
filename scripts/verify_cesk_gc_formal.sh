#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$SCRIPT_DIR/.." && pwd)"
TLA_DIR="$REPO/tla"
TLC_META="${TLC_META:-$REPO/target/tlc-formal-small}"

mkdir -p "$TLC_META"

run_lean() {
  local file="$1"
  echo "### Lean: $file"
  lean "$REPO/$file"
}

run_rocq() {
  local file="$1"
  echo "### Rocq: $file"
  (
    cd "$REPO"
    systemd-run --user --scope \
      -p MemoryMax=4G -p MemorySwapMax=0 -p CPUQuota=200% --quiet \
      rocq c -q "$file"
  )
}

run_source_coupling() {
  echo "### Source coupling: CESK GC"
  bash "$REPO/scripts/verify_cesk_gc_source_coupling.sh"
}

run_tlc() {
  local label="$1" module="$2" cfg="$3" expect="$4" pattern="$5"
  local log="$TLC_META/${label}.log"
  local run_meta
  run_meta="$(mktemp -d "$TLC_META/${label}.XXXXXX")"

  echo "### TLC: $label"
  set +e
  (
    cd "$TLA_DIR"
    systemd-run --user --scope \
      -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400% --quiet \
      tlc -metadir "$run_meta" -workers auto "$module" -config "$cfg"
  ) >"$log" 2>&1
  local rc=$?
  set -e

  case "$expect" in
    pass)
      if [[ "$rc" -ne 0 ]]; then
        tail -120 "$log"
        echo "TLC $label expected pass, got rc=$rc" >&2
        return 1
      fi
      if ! grep -q "No error has been found" "$log"; then
        tail -120 "$log"
        echo "TLC $label did not report success" >&2
        return 1
      fi
      ;;
    fail)
      if [[ "$rc" -eq 0 ]]; then
        tail -120 "$log"
        echo "TLC $label expected failure, got success" >&2
        return 1
      fi
      if ! grep -q "$pattern" "$log"; then
        tail -120 "$log"
        echo "TLC $label failed, but not with expected discriminator: $pattern" >&2
        return 1
      fi
      ;;
    *)
      echo "invalid expectation: $expect" >&2
      return 2
      ;;
  esac
}

run_lean "formal/lean/gc/FreeList.lean"
run_lean "formal/lean/gc/YoungMark.lean"
run_lean "formal/lean/gc/StructuralRoots.lean"
run_lean "formal/lean/gc/RendezvousWitness.lean"

run_rocq "formal/rocq/gc/FreeList.v"
run_rocq "formal/rocq/gc/YoungMark.v"
run_rocq "formal/rocq/gc/StructuralRoots.v"
run_rocq "formal/rocq/gc/RendezvousWitness.v"
run_rocq "formal/rocq/gc/CESKCollectorSafety.v"

run_source_coupling

run_tlc "rfl_freebit" "MC_StoreCentricGC_RFL.tla" "MC_RFL_freebit.cfg" \
  pass ""
run_tlc "rfl_bug" "MC_StoreCentricGC_RFL.tla" "MC_RFL_bug.cfg" \
  fail "Invariant NoDuplicateFreeListEntries is violated"
run_tlc "collapse_fix" "CollapseCompletion.tla" "CollapseCompletion_fix.cfg" \
  pass ""
run_tlc "collapse_bug" "CollapseCompletion.tla" "CollapseCompletion_bug.cfg" \
  fail "Temporal properties were violated"
run_tlc "rendezvous_witness_strict" "MC_RendezvousWitness.tla" "MC_RendezvousWitness_strict.cfg" \
  pass ""
run_tlc "rendezvous_witness_weak" "MC_RendezvousWitness.tla" "MC_RendezvousWitness_weak.cfg" \
  fail "Invariant RootCompleteOnSweep is violated"
run_tlc "witness_slot_lifecycle_v4" "MC_WitnessSlotLifecycle.tla" "MC_WitnessSlotLifecycle_v4.cfg" \
  pass ""
run_tlc "witness_slot_lifecycle_bug" "MC_WitnessSlotLifecycle.tla" "MC_WitnessSlotLifecycle_bug.cfg" \
  fail "Invariant LiveMachineVisibleOnSweep is violated"
run_tlc "driver_root_union_all" "MC_DriverRootUnion.tla" "MC_DriverRootUnion_all.cfg" \
  pass ""
run_tlc "driver_root_union_missing_env" "MC_DriverRootUnion.tla" "MC_DriverRootUnion_missing_env.cfg" \
  fail "Invariant RootUnionComplete is violated"
run_tlc "driver_root_union_missing_dispatch" "MC_DriverRootUnion.tla" "MC_DriverRootUnion_missing_dispatch.cfg" \
  fail "Invariant RootUnionComplete is violated"

echo "CESK GC formal checks passed"
