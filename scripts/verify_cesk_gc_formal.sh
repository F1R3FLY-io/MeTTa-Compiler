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
run_lean "formal/lean/gc/ThreadContribution.lean"
run_lean "formal/lean/gc/DriverCPublication.lean"
run_lean "formal/lean/gc/BatchHandoff.lean"
run_lean "formal/lean/gc/SATB.lean"
run_lean "formal/lean/gc/AllocateBlack.lean"
run_lean "formal/lean/gc/SATBFinalization.lean"
run_lean "formal/lean/gc/FullMajorSweep.lean"
run_lean "formal/lean/gc/E0MutationSites.lean"
run_lean "formal/lean/gc/E0EvictionBarriers.lean"

run_rocq "formal/rocq/gc/FreeList.v"
run_rocq "formal/rocq/gc/YoungMark.v"
run_rocq "formal/rocq/gc/StructuralRoots.v"
run_rocq "formal/rocq/gc/RendezvousWitness.v"
run_rocq "formal/rocq/gc/ThreadContribution.v"
run_rocq "formal/rocq/gc/DriverCPublication.v"
run_rocq "formal/rocq/gc/BatchHandoff.v"
run_rocq "formal/rocq/gc/SATB.v"
run_rocq "formal/rocq/gc/AllocateBlack.v"
run_rocq "formal/rocq/gc/SATBFinalization.v"
run_rocq "formal/rocq/gc/FullMajorSweep.v"
run_rocq "formal/rocq/gc/E0MutationSites.v"
run_rocq "formal/rocq/gc/E0EvictionBarriers.v"
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
run_tlc "driver_c_publication_published" "MC_DriverCPublication.tla" "MC_DriverCPublication_published.cfg" \
  pass ""
run_tlc "driver_c_publication_missing" "MC_DriverCPublication.tla" "MC_DriverCPublication_missing.cfg" \
  fail "Invariant DriverCVisibleOnSweep is violated"
run_tlc "batch_handoff_handle" "MC_BatchHandoff.tla" "MC_BatchHandoff_handle.cfg" \
  pass ""
run_tlc "batch_handoff_no_handle" "MC_BatchHandoff.tla" "MC_BatchHandoff_no_handle.cfg" \
  fail "Invariant NoPublishedBatchResultFreed is violated"
run_tlc "batch_handoff_drop_before_copy" "MC_BatchHandoff.tla" "MC_BatchHandoff_drop_before_copy.cfg" \
  fail "Invariant NoPublishedBatchResultFreed is violated"
run_tlc "started_cycle_gate_started" "MC_StartedCycleGate.tla" "MC_StartedCycleGate_started.cfg" \
  pass ""
run_tlc "started_cycle_gate_gen" "MC_StartedCycleGate.tla" "MC_StartedCycleGate_gen.cfg" \
  fail "Invariant NoPhantomRepark is violated"
run_tlc "witness_ok_reset_clear" "MC_WitnessOkReset.tla" "MC_WitnessOkReset_clear.cfg" \
  pass ""
run_tlc "witness_ok_reset_stale" "MC_WitnessOkReset.tla" "MC_WitnessOkReset_stale.cfg" \
  fail "Invariant NoStaleWitnessCollect is violated"
run_tlc "cur_seg_reuse_order_cur" "MC_CurSegReuseOrder.tla" "MC_CurSegReuseOrder_cur.cfg" \
  pass ""
run_tlc "cur_seg_reuse_order_any" "MC_CurSegReuseOrder.tla" "MC_CurSegReuseOrder_any.cfg" \
  fail "Invariant NoOldToYoungAfterPromotion is violated"
run_tlc "conservative_minor_mark" "MC_ConservativeMinorMark.tla" "MC_ConservativeMinorMark_conservative.cfg" \
  pass ""
run_tlc "conservative_minor_skip_old" "MC_ConservativeMinorMark.tla" "MC_ConservativeMinorMark_skip_old.cfg" \
  fail "Invariant YoungReachableMarked is violated"
run_tlc "satb_deletion_barrier" "MC_SATBDeletionBarrier.tla" "MC_SATBDeletionBarrier_satb.cfg" \
  pass ""
run_tlc "satb_no_barrier" "MC_SATBDeletionBarrier.tla" "MC_SATBDeletionBarrier_none.cfg" \
  fail "Invariant NoSnapshotLiveFreed is violated"
run_tlc "satb_e0_mutation_sites_all" "MC_SATBE0MutationSites.tla" "MC_SATBE0MutationSites_all.cfg" \
  pass ""
run_tlc "satb_e0_mutation_sites_no_space" "MC_SATBE0MutationSites.tla" "MC_SATBE0MutationSites_no_space.cfg" \
  fail "Invariant NoE0SnapshotLiveFreed is violated"
run_tlc "satb_e0_mutation_sites_no_rule" "MC_SATBE0MutationSites.tla" "MC_SATBE0MutationSites_no_rule.cfg" \
  fail "Invariant NoE0SnapshotLiveFreed is violated"
run_tlc "satb_e0_mutation_sites_no_env" "MC_SATBE0MutationSites.tla" "MC_SATBE0MutationSites_no_env.cfg" \
  fail "Invariant NoE0SnapshotLiveFreed is violated"
run_tlc "allocate_black_mark_first" "MC_AllocateBlackPublish.tla" "MC_AllocateBlackPublish_mark_first.cfg" \
  pass ""
run_tlc "allocate_black_publish_first" "MC_AllocateBlackPublish.tla" "MC_AllocateBlackPublish_publish_first.cfg" \
  fail "Invariant NoPublishedAllocSwept is violated"
run_tlc "satb_lru_capacity" "MC_SATBLRUEviction.tla" "MC_SATBLRUEviction_capacity.cfg" \
  pass ""
run_tlc "satb_lru_put_return" "MC_SATBLRUEviction.tla" "MC_SATBLRUEviction_put_return.cfg" \
  fail "Invariant NoSnapshotVictimFreed is violated"
run_tlc "satb_bulk_clear_shade" "MC_SATBBulkClear.tla" "MC_SATBBulkClear_shade.cfg" \
  pass ""
run_tlc "satb_bulk_clear_none" "MC_SATBBulkClear.tla" "MC_SATBBulkClear_none.cfg" \
  fail "Invariant NoSnapshotClearedEntryFreed is violated"
run_tlc "satb_phase_gate" "MC_SATBPhaseGate.tla" "MC_SATBPhaseGate_gate.cfg" \
  pass ""
run_tlc "satb_phase_race" "MC_SATBPhaseGate.tla" "MC_SATBPhaseGate_race.cfg" \
  fail "Invariant NoSnapshotLiveFreed is violated"
run_tlc "satb_sweep_gate" "MC_SATBSweepGate.tla" "MC_SATBSweepGate_gate.cfg" \
  pass ""
run_tlc "satb_sweep_race" "MC_SATBSweepGate.tla" "MC_SATBSweepGate_race.cfg" \
  fail "Invariant NoSnapshotLiveFreed is violated"
run_tlc "satb_final_remark" "MC_SATBFinalRemark.tla" "MC_SATBFinalRemark_remark.cfg" \
  pass ""
run_tlc "satb_no_final_remark" "MC_SATBFinalRemark.tla" "MC_SATBFinalRemark_none.cfg" \
  fail "Invariant NoFinalRootFreed is violated"
run_tlc "satb_final_sweep_result_checked" "MC_SATBFinalSweepResult.tla" "MC_SATBFinalSweepResult_checked.cfg" \
  pass ""
run_tlc "satb_final_sweep_result_unchecked" "MC_SATBFinalSweepResult.tla" "MC_SATBFinalSweepResult_unchecked.cfg" \
  fail "Invariant FinalSweepHandled is violated"
run_tlc "satb_abort_fallback" "MC_SATBAbortFallback.tla" "MC_SATBAbortFallback_stw.cfg" \
  pass ""
run_tlc "satb_abort_no_fallback" "MC_SATBAbortFallback.tla" "MC_SATBAbortFallback_none.cfg" \
  fail "Invariant AbortHasBackstop is violated"
run_tlc "satb_abort_no_request" "MC_SATBAbortFallback.tla" "MC_SATBAbortFallback_no_request.cfg" \
  fail "Invariant FallbackSTWRequested is violated"

echo "CESK GC formal checks passed"
