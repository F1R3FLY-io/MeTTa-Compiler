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

run_lean_mirrors() {
  local file rel
  while IFS= read -r file; do
    rel="${file#$REPO/}"
    run_lean "$rel"
  done < <(find "$REPO/formal/lean/gc" -maxdepth 1 -type f -name '*.lean' | sort)
}

run_rocq() {
  local file="$1"
  echo "### Rocq: $file"
  (
    cd "$REPO"
    systemd-run --user --scope \
      -p MemoryMax=4G -p MemorySwapMax=0 -p CPUQuota=200% --quiet \
      rocq c -q -Q formal/rocq/gc "" "$file"
  )
}

run_workpool_rocq() {
  local file="$1"
  echo "### Rocq WorkPoolStability: $file"
  (
    cd "$REPO"
    systemd-run --user --scope \
      -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=200% --quiet \
      rocq c -q -Q formal/rocq/work_pool_stability/theories WorkPoolStability "$file"
  )
}

run_source_coupling() {
  echo "### Source coupling: CESK GC"
  bash "$REPO/scripts/verify_cesk_gc_source_coupling.sh"
}

run_proof_hygiene() {
  echo "### Proof hygiene: CESK GC"
  bash "$REPO/scripts/verify_cesk_gc_proof_hygiene.sh"
}

run_tlc_hygiene() {
  echo "### TLC hygiene: CESK GC"
  bash "$REPO/scripts/verify_cesk_gc_tlc_hygiene.sh"
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

run_proof_hygiene
run_tlc_hygiene

run_lean_mirrors

run_workpool_rocq "formal/rocq/work_pool_stability/theories/Prelude.v"
run_workpool_rocq "formal/rocq/work_pool_stability/theories/USL.v"
run_workpool_rocq "formal/rocq/work_pool_stability/theories/ObjectiveFunction.v"
run_workpool_rocq "formal/rocq/work_pool_stability/theories/LyapunovConvergence.v"
run_workpool_rocq "formal/rocq/work_pool_stability/theories/WeightDominance.v"

run_rocq "formal/rocq/gc/FreeList.v"
run_rocq "formal/rocq/gc/YoungMark.v"
run_rocq "formal/rocq/gc/NurseryBackpressure.v"
run_rocq "formal/rocq/gc/DeepBranchingCollectionProgress.v"
run_rocq "formal/rocq/gc/YoungAllocationOdometer.v"
run_rocq "formal/rocq/gc/MajorMinorScheduler.v"
run_rocq "formal/rocq/gc/CapFloorAntiThrash.v"
run_rocq "formal/rocq/gc/MajorWatermarkRearm.v"
# F1 SATB-young lever: the pruning-mark stale-old-mark discriminator + the
# ClearOldMarks=TRUE completeness/NoStaleOldMark obligations (the Rocq side of
# tla/SATBYoungSweepStaleOldMark.tla; wired to IndexArena::clear_old_marks and
# the rendezvous-phase minor arm).
run_rocq "formal/rocq/gc/StaleOldMarkClear.v"
run_rocq "formal/rocq/gc/StructuralRoots.v"
run_rocq "formal/rocq/gc/StructuralRootSourceAudit.v"
run_rocq "formal/rocq/gc/TrackedVarSideRetention.v"
# Finding 2 (correct-by-construction): interned atom bytes are never freed, so
# as_atom's &'static is HONEST and the laundered-&str UAF class is impossible. Verified
# AHEAD of the interning implementation (per formal-method-first); the source-coupling
# pins binding it to alloc_atom/intern_static land with that implementation.
run_rocq "formal/rocq/gc/InternedAtomNeverFreed.v"
run_rocq "formal/rocq/gc/AtomDedupMemoSoundness.v"
run_rocq "formal/rocq/gc/RegistryIsolation.v"
run_rocq "formal/rocq/gc/NodeEdgeCompleteness.v"
run_rocq "formal/rocq/gc/AbstractGCLiveNarrowing.v"
run_rocq "formal/rocq/gc/MidloopRootUnion.v"
run_rocq "formal/rocq/gc/KSpineCurrentWork.v"
run_rocq "formal/rocq/gc/VmNestedLocals.v"
run_rocq "formal/rocq/gc/NonRendezvousFanoutGate.v"
run_rocq "formal/rocq/gc/RendezvousWitness.v"
run_rocq "formal/rocq/gc/WitnessSlotLifecycle.v"
run_rocq "formal/rocq/gc/WitnessOkReset.v"
run_rocq "formal/rocq/gc/GenerationResume.v"
run_rocq "formal/rocq/gc/RendezvousProgress.v"
run_rocq "formal/rocq/gc/StartedCycleGate.v"
# Bug #309: the phantom-future park gate (the branch-B/pump twin of the straddle gate).
run_rocq "formal/rocq/gc/ParkedPhantomCycleGate.v"
run_rocq "formal/rocq/gc/RequestReassertAtOpen.v"
# E5: liveness of the SATB double-rendezvous straddle (composes GenerationResume +
# StartedCycleGate via Require Import; the -Q loadpath above resolves the siblings).
run_rocq "formal/rocq/gc/PostCycleEvaluatorProgress.v"
run_rocq "formal/rocq/gc/CollapseCompletion.v"
run_rocq "formal/rocq/gc/BindingProjection.v"
run_rocq "formal/rocq/gc/WorkerAdmission.v"
run_rocq "formal/rocq/gc/ThreadContribution.v"
run_rocq "formal/rocq/gc/FrameEnvRoots.v"
run_rocq "formal/rocq/gc/TierLeafExtraRoots.v"
run_rocq "formal/rocq/gc/SelectiveChoicePointRoots.v"
run_rocq "formal/rocq/gc/StoredBranchCoroutineSpine.v"
run_rocq "formal/rocq/gc/VmChoicePointSpine.v"
run_rocq "formal/rocq/gc/JitChoicePointSpineBridge.v"
run_rocq "formal/rocq/gc/JitStackSavePoolFreshness.v"
run_rocq "formal/rocq/gc/JitChoicePointProductionRestore.v"
run_rocq "formal/rocq/gc/TrampolineFanoutSpineBridge.v"
run_rocq "formal/rocq/gc/TrampolineFanoutProductionRestore.v"
run_rocq "formal/rocq/gc/UnifiedChoicePointRestore.v"
run_rocq "formal/rocq/gc/TrampolineFanoutSpineProgress.v"
run_rocq "formal/rocq/gc/IncrementalSpinePersistEquivalence.v"
run_rocq "formal/rocq/gc/SerializableContinuationSlice.v"
run_rocq "formal/rocq/gc/IndexArenaPublication.v"
run_rocq "formal/rocq/gc/SideArenaPublication.v"
run_rocq "formal/rocq/gc/InnerColumnReadRefinement.v"
run_rocq "formal/rocq/gc/ConcurrentBumpFreshOnly.v"
run_rocq "formal/rocq/gc/ConcurrentReusePressureProgress.v"
run_rocq "formal/rocq/gc/IndexAllocatorRefinement.v"
run_rocq "formal/rocq/gc/SideFreeQuiescence.v"
run_rocq "formal/rocq/gc/SideReclaimRefinement.v"
run_rocq "formal/rocq/gc/QuiescentSideIndexReuse.v"
# #273: bounded side-payload reclaim progress under the rendezvous collector (design B:
# pending_side_major forces a quiescence drain; the drain is exhaustive mem::take).
run_rocq "formal/rocq/gc/RendezvousSideReclaimProgress.v"
run_rocq "formal/rocq/gc/HashConsSweepRetain.v"
run_rocq "formal/rocq/gc/DriverRootUnion.v"
run_rocq "formal/rocq/gc/DriverCPublication.v"
run_rocq "formal/rocq/gc/BatchHandoff.v"
run_rocq "formal/rocq/gc/RholangBatchCompletion.v"
run_rocq "formal/rocq/gc/SchedulerGcBoundary.v"
run_rocq "formal/rocq/gc/SchedulerSpawnLatch.v"
run_rocq "formal/rocq/gc/SchedulerFanoutProgress.v"
run_rocq "formal/rocq/gc/SchedulerFanoutAdmissionCompleteness.v"
run_rocq "formal/rocq/gc/SchedulerActiveFanoutGate.v"
run_rocq "formal/rocq/gc/CollapseFanoutAdmissionCompleteness.v"
run_rocq "formal/rocq/gc/SchedulerWavefrontParallelism.v"
run_rocq "formal/rocq/gc/SchedulerEffectConflictCompleteness.v"
run_rocq "formal/rocq/gc/SchedulerDirectFanoutWavefrontRefinement.v"
run_rocq "formal/rocq/gc/SchedulerTransducerParallelism.v"
run_rocq "formal/rocq/gc/SchedulerDynamicEvalGate.v"
run_rocq "formal/rocq/gc/SchedulerPriorityFairness.v"
run_rocq "formal/rocq/gc/SchedulerClassificationLookup.v"
run_rocq "formal/rocq/gc/CronRecurringDispatch.v"
run_rocq "formal/rocq/gc/WorkPoolOverflowCap.v"
run_rocq "formal/rocq/gc/WorkPoolLifecycle.v"
run_rocq "formal/rocq/gc/WorkPoolStartupDrain.v"
run_rocq "formal/rocq/gc/WorkPoolPanicIsolation.v"
run_rocq "formal/rocq/gc/CounterFlushExclusion.v"
run_rocq "formal/rocq/gc/GcDriverChannelProtocol.v"
run_rocq "formal/rocq/gc/StructChannelPairing.v"
run_rocq "formal/rocq/gc/JitCacheEntryThreadSafety.v"
run_rocq "formal/rocq/gc/DedicatedHandoff.v"
run_rocq "formal/rocq/gc/DedicatedSingleRegime.v"
run_rocq "formal/rocq/gc/DepthZeroSafepoint.v"
run_rocq "formal/rocq/gc/PollEdgeContribution.v"
run_rocq "formal/rocq/gc/ConcurrentTriggerBackstop.v"
run_rocq "formal/rocq/gc/E1DefaultConcurrentFlip.v"
run_rocq "formal/rocq/gc/E1SatbStwDriverProgress.v"
run_rocq "formal/rocq/gc/DefaultStoreSelection.v"
run_rocq "formal/rocq/gc/JitValueCreationStoreSelection.v"
run_rocq "formal/rocq/gc/ArenaAddrDecodeErasure.v"
run_rocq "formal/rocq/gc/CfgGuardErasure.v"
run_rocq "formal/rocq/gc/InnerPtrDecodeErasure.v"
run_rocq "formal/rocq/gc/CronProducerErasure.v"
run_rocq "formal/rocq/gc/GcPoolErasure.v"
run_rocq "formal/rocq/gc/RootDiscoveryErasure.v"
run_rocq "formal/rocq/gc/RuntimeModeErasure.v"
run_rocq "formal/rocq/gc/OperatorCacheEpoch.v"
run_rocq "formal/rocq/gc/EpochProtectedCaches.v"
run_rocq "formal/rocq/gc/WriteOnceAnchors.v"
run_rocq "formal/rocq/gc/SpaceRegistryBarriers.v"
run_rocq "formal/rocq/gc/TieredCacheBarriers.v"
run_rocq "formal/rocq/gc/ThreadLocalTablesBarriers.v"
run_rocq "formal/rocq/gc/E0CacheBarrierCompleteness.v"
run_rocq "formal/rocq/gc/SATB.v"
run_rocq "formal/rocq/gc/SATBGates.v"
run_rocq "formal/rocq/gc/SATBTriggerSuppression.v"
run_rocq "formal/rocq/gc/AllocateBlack.v"
run_rocq "formal/rocq/gc/SATBFinalization.v"
run_rocq "formal/rocq/gc/SATBAbortFallback.v"
run_rocq "formal/rocq/gc/FullMajorSweep.v"
run_rocq "formal/rocq/gc/E0MutationSites.v"
run_rocq "formal/rocq/gc/E0EvictionBarriers.v"
run_rocq "formal/rocq/gc/SATBSubmodelClosure.v"
run_rocq "formal/rocq/gc/CESKCollectorSafety.v"

run_source_coupling

run_tlc "priority_queue_aging_refresh" "PriorityQueueAging.tla" "MC_PriorityQueueAging_refresh.cfg" \
  pass ""
run_tlc "priority_queue_aging_stale" "PriorityQueueAging.tla" "MC_PriorityQueueAging_stale.cfg" \
  fail "Invariant OldPopsAfterAging is violated"
run_tlc "scheduler_classification_lookup_shift" "SchedulerClassificationLookup.tla" "MC_SchedulerClassificationLookup_shift.cfg" \
  pass ""
run_tlc "scheduler_classification_lookup_no_shift" "SchedulerClassificationLookup.tla" "MC_SchedulerClassificationLookup_no_shift.cfg" \
  fail "Invariant RangesDisjoint is violated"
run_tlc "scheduler_wavefront_diamond" "SchedulerWavefrontParallelism.tla" "MC_SchedulerWavefrontParallelism_diamond.cfg" \
  pass ""
run_tlc "scheduler_wavefront_independent" "SchedulerWavefrontParallelism.tla" "MC_SchedulerWavefrontParallelism_independent.cfg" \
  pass ""
run_tlc "scheduler_wavefront_cycle_same_wave" "SchedulerWavefrontParallelism.tla" "MC_SchedulerWavefrontParallelism_cycle.cfg" \
  fail "The invariant of SameWaveIndependent is equal to FALSE"
run_tlc "scheduler_wavefront_deferred_ready" "SchedulerWavefrontParallelism.tla" "MC_SchedulerWavefrontParallelism_deferred.cfg" \
  fail "The invariant of NoReadyTaskDeferred is equal to FALSE"
run_tlc "scheduler_effect_conflict_complete" "SchedulerEffectConflictCompleteness.tla" "MC_SchedulerEffectConflictCompleteness_complete.cfg" \
  pass ""
run_tlc "scheduler_effect_conflict_no_conflicts" "SchedulerEffectConflictCompleteness.tla" "MC_SchedulerEffectConflictCompleteness_no_conflicts.cfg" \
  pass ""
run_tlc "scheduler_effect_conflict_missing_edge" "SchedulerEffectConflictCompleteness.tla" "MC_SchedulerEffectConflictCompleteness_missing_edge.cfg" \
  fail "The invariant of ConflictEdgesCovered is equal to FALSE"
run_tlc "scheduler_transducer_zero_cap" "SchedulerTransducerParallelism.tla" "MC_SchedulerTransducerParallelism_zero_cap.cfg" \
  pass ""
run_tlc "scheduler_transducer_zero_cap_bug" "SchedulerTransducerParallelism.tla" "MC_SchedulerTransducerParallelism_zero_cap_bug.cfg" \
  fail "The invariant of NonZeroDegree is equal to FALSE"
run_tlc "scheduler_transducer_underutilized" "SchedulerTransducerParallelism.tla" "MC_SchedulerTransducerParallelism_underutilized.cfg" \
  fail "The invariant of MaximalBeforeCap is equal to FALSE"
run_tlc "scheduler_fanout_admission_all" "SchedulerFanoutAdmissionCompleteness.tla" "MC_SchedulerFanoutAdmissionCompleteness_all.cfg" \
  pass ""
run_tlc "scheduler_fanout_admission_partial" "SchedulerFanoutAdmissionCompleteness.tla" "MC_SchedulerFanoutAdmissionCompleteness_partial.cfg" \
  fail "The invariant of CompleteAdmittedFanout is equal to FALSE"
run_tlc "scheduler_fanout_admission_missing_degree" "SchedulerFanoutAdmissionCompleteness.tla" "MC_SchedulerFanoutAdmissionCompleteness_missing_degree.cfg" \
  fail "The invariant of DegreeGateRequired is equal to FALSE"
run_tlc "scheduler_active_fanout_all" "SchedulerActiveFanoutGate.tla" "MC_SchedulerActiveFanoutGate_all.cfg" \
  pass ""
run_tlc "scheduler_active_fanout_missing_purity" "SchedulerActiveFanoutGate.tla" "MC_SchedulerActiveFanoutGate_missing_purity.cfg" \
  fail "The invariant of NoDispatchWithoutPurityGate is equal to FALSE"
run_tlc "scheduler_active_fanout_missing_budget" "SchedulerActiveFanoutGate.tla" "MC_SchedulerActiveFanoutGate_missing_budget.cfg" \
  fail "The invariant of NoDispatchWithoutBudgetGate is equal to FALSE"
run_tlc "scheduler_active_fanout_partial_dispatch" "SchedulerActiveFanoutGate.tla" "MC_SchedulerActiveFanoutGate_partial_dispatch.cfg" \
  fail "The invariant of CompleteDispatch is equal to FALSE"
run_tlc "jit_value_creation_index_correct" "JitValueCreationStoreSelection.tla" "MC_JitValueCreationStoreSelection_index_correct.cfg" \
  pass ""
run_tlc "jit_value_creation_index_slab_bug" "JitValueCreationStoreSelection.tla" "MC_JitValueCreationStoreSelection_index_slab_bug.cfg" \
  fail "The invariant of FactoryMatchesCompiledStore is equal to FALSE"
run_tlc "jit_value_creation_index_arena_bug" "JitValueCreationStoreSelection.tla" "MC_JitValueCreationStoreSelection_index_arena_bug.cfg" \
  fail "The invariant of NoArenaPtrStoreSelectionInIndex is equal to FALSE"
run_tlc "jit_value_creation_legacy_slab_correct" "JitValueCreationStoreSelection.tla" "MC_JitValueCreationStoreSelection_legacy_slab_correct.cfg" \
  pass ""
run_tlc "scheduler_direct_fanout_wavefront_independent" "SchedulerDirectFanoutWavefrontRefinement.tla" "MC_SchedulerDirectFanoutWavefrontRefinement_independent.cfg" \
  pass ""
run_tlc "scheduler_direct_fanout_wavefront_dependent_missing_gate" "SchedulerDirectFanoutWavefrontRefinement.tla" "MC_SchedulerDirectFanoutWavefrontRefinement_dependent_missing_gate.cfg" \
  fail "The invariant of DirectOnlyForIndependentWavefront is equal to FALSE"
run_tlc "scheduler_direct_fanout_wavefront_partial_dispatch" "SchedulerDirectFanoutWavefrontRefinement.tla" "MC_SchedulerDirectFanoutWavefrontRefinement_partial_dispatch.cfg" \
  fail "The invariant of DirectMatchesWavefrontMaxParallelism is equal to FALSE"
run_tlc "collapse_fanout_admission_all" "CollapseFanoutAdmissionCompleteness.tla" "MC_CollapseFanoutAdmissionCompleteness_all.cfg" \
  pass ""
run_tlc "collapse_fanout_admission_partial" "CollapseFanoutAdmissionCompleteness.tla" "MC_CollapseFanoutAdmissionCompleteness_partial.cfg" \
  fail "The invariant of CompleteAdmittedCollapse is equal to FALSE"
run_tlc "collapse_fanout_admission_missing_threshold" "CollapseFanoutAdmissionCompleteness.tla" "MC_CollapseFanoutAdmissionCompleteness_missing_threshold.cfg" \
  fail "The invariant of ThresholdGateRequired is equal to FALSE"
run_tlc "scheduler_dynamic_eval_gate_fixed" "SchedulerDynamicEvalGate.tla" "MC_SchedulerDynamicEvalGate_fixed.cfg" \
  pass ""
run_tlc "scheduler_dynamic_eval_gate_missing" "SchedulerDynamicEvalGate.tla" "MC_SchedulerDynamicEvalGate_missing.cfg" \
  fail "The invariant of NoDynamicEvalParallelBypass is equal to FALSE"
run_tlc "cron_recurring_dispatch_stop" "CronRecurringDispatch.tla" "MC_CronRecurringDispatch_stop.cfg" \
  pass ""
run_tlc "cron_recurring_dispatch_no_stop" "CronRecurringDispatch.tla" "MC_CronRecurringDispatch_no_stop.cfg" \
  fail "Invariant StopPreventsRedispatch is violated"
run_tlc "cron_recurring_dispatch_no_claim" "CronRecurringDispatch.tla" "MC_CronRecurringDispatch_no_claim.cfg" \
  fail "Invariant NoOverlapDispatch is violated"
run_tlc "work_pool_overflow_capped" "WorkPoolOverflowCap.tla" "MC_WorkPoolOverflowCap_capped.cfg" \
  pass ""
run_tlc "work_pool_overflow_uncapped" "WorkPoolOverflowCap.tla" "MC_WorkPoolOverflowCap_uncapped.cfg" \
  fail "Invariant LiveWithinCap is violated"
run_tlc "work_pool_lifecycle_fixed" "WorkPoolLifecycle.tla" "MC_WorkPoolLifecycle_fixed.cfg" \
  pass ""
run_tlc "work_pool_lifecycle_double_unpark_bug" "WorkPoolLifecycle.tla" "MC_WorkPoolLifecycle_double_unpark_bug.cfg" \
  fail "Invariant CapacityConsistent is violated"
run_tlc "work_pool_lifecycle_respawn_bug" "WorkPoolLifecycle.tla" "MC_WorkPoolLifecycle_respawn_bug.cfg" \
  fail "Invariant CapacityConsistent is violated"
run_tlc "work_pool_startup_drain_all" "WorkPoolStartupDrain.tla" "WorkPoolStartupDrain_all.cfg" \
  pass ""
run_tlc "work_pool_startup_drain_no_start" "WorkPoolStartupDrain.tla" "WorkPoolStartupDrain_no_start.cfg" \
  fail "Temporal properties were violated"
run_tlc "work_pool_startup_drain_lossy_enqueue" "WorkPoolStartupDrain.tla" "WorkPoolStartupDrain_lossy_enqueue.cfg" \
  fail "Invariant AllSubmittedComplete is violated"
run_tlc "work_pool_panic_task_caught" "WorkPoolPanicIsolation.tla" "WorkPoolPanicIsolation_task_caught.cfg" \
  pass ""
run_tlc "work_pool_panic_accounting_caught" "WorkPoolPanicIsolation.tla" "WorkPoolPanicIsolation_accounting_caught.cfg" \
  pass ""
run_tlc "work_pool_panic_no_inner" "WorkPoolPanicIsolation.tla" "WorkPoolPanicIsolation_no_inner.cfg" \
  fail "Invariant TaskPanicPublishesHeartbeat is violated"
run_tlc "work_pool_panic_no_outer" "WorkPoolPanicIsolation.tla" "WorkPoolPanicIsolation_no_outer.cfg" \
  fail "Temporal properties were violated"
run_tlc "counter_flush_locked" "CounterFlushExclusion.tla" "MC_CounterFlushExclusion_locked.cfg" \
  pass ""
run_tlc "counter_flush_unlocked" "CounterFlushExclusion.tla" "MC_CounterFlushExclusion_unlocked.cfg" \
  fail "Invariant NoCounterSyncFreeOverlap is violated"
run_tlc "gc_driver_channel_protocol_paired" "GcDriverChannelProtocol.tla" "MC_GcDriverChannelProtocol_paired.cfg" \
  pass ""
run_tlc "gc_driver_channel_protocol_no_request_sender" "GcDriverChannelProtocol.tla" "MC_GcDriverChannelProtocol_no_request_sender.cfg" \
  fail "Invariant RequestReceiveHasProducer is violated"
run_tlc "gc_driver_channel_protocol_no_reply" "GcDriverChannelProtocol.tla" "MC_GcDriverChannelProtocol_no_reply.cfg" \
  fail "Invariant ResponseWaitHasProducer is violated"
run_tlc "gc_driver_channel_protocol_orphan_reply" "GcDriverChannelProtocol.tla" "MC_GcDriverChannelProtocol_orphan_reply.cfg" \
  fail "Invariant NoOrphanReplySend is violated"
run_tlc "struct_channel_pairing_paired" "StructChannelPairing.tla" "MC_StructChannelPairing_paired.cfg" \
  pass ""
run_tlc "struct_channel_pairing_no_sender" "StructChannelPairing.tla" "MC_StructChannelPairing_no_sender.cfg" \
  fail "Invariant StoredReceiveHasProducer is violated"
run_tlc "struct_channel_pairing_no_worker_clone" "StructChannelPairing.tla" "MC_StructChannelPairing_no_worker_clone.cfg" \
  fail "Invariant WorkerReceiveHasProducer is violated"
run_tlc "struct_channel_pairing_no_response_receiver" "StructChannelPairing.tla" "MC_StructChannelPairing_no_response_receiver.cfg" \
  fail "Invariant ResponseSendHasReceiver is violated"
run_tlc "struct_channel_pairing_no_ready_sender" "StructChannelPairing.tla" "MC_StructChannelPairing_no_ready_sender.cfg" \
  fail "Invariant ReadyWaitHasSignal is violated"

run_tlc "rfl_freebit" "MC_StoreCentricGC_RFL.tla" "MC_RFL_freebit.cfg" \
  pass ""
run_tlc "rfl_bug" "MC_StoreCentricGC_RFL.tla" "MC_RFL_bug.cfg" \
  fail "Invariant NoDuplicateFreeListEntries is violated"
run_tlc "generational_full_mark_small" "MC_StoreCentricGC_Generational.tla" "MC_StoreCentricGC_Generational_small.cfg" \
  pass ""
run_tlc "gen_young_mark_curseg_small" "MC_StoreCentricGC_GenerationalYoungMark.tla" "MC_GenYoungMark_positive_small.cfg" \
  pass ""
run_tlc "gen_young_mark_any_small" "MC_StoreCentricGC_GenerationalYoungMark.tla" "MC_GenYoungMark_negative_small.cfg" \
  fail "Invariant YoungOnlyMarkReachesLiveYoung is violated"
run_tlc "node_edge_completeness_all" "MC_NodeEdgeCompleteness.tla" "MC_NodeEdgeCompleteness_all.cfg" \
  pass ""
run_tlc "node_edge_completeness_missing_inline" "MC_NodeEdgeCompleteness.tla" "MC_NodeEdgeCompleteness_missing_inline.cfg" \
  fail "Invariant NoReachableFreed is violated"
run_tlc "node_edge_completeness_missing_side" "MC_NodeEdgeCompleteness.tla" "MC_NodeEdgeCompleteness_missing_side.cfg" \
  fail "Invariant NoReachableFreed is violated"
run_tlc "node_edge_completeness_missing_space" "MC_NodeEdgeCompleteness.tla" "MC_NodeEdgeCompleteness_missing_space.cfg" \
  fail "Invariant NoReachableFreed is violated"
run_tlc "abstract_gc_live_narrowing_all_dead" "MC_AbstractGCLiveNarrowing.tla" "MC_AbstractGCLiveNarrowing_all_dead.cfg" \
  pass ""
run_tlc "abstract_gc_live_narrowing_rule_matches_live" "MC_AbstractGCLiveNarrowing.tla" "MC_AbstractGCLiveNarrowing_rule_matches_live.cfg" \
  fail "Invariant NoFutureTouchFreed is violated"
run_tlc "abstract_gc_live_narrowing_alts_live" "MC_AbstractGCLiveNarrowing.tla" "MC_AbstractGCLiveNarrowing_alts_live.cfg" \
  fail "Invariant NoFutureTouchFreed is violated"
run_tlc "abstract_gc_live_narrowing_templates_live" "MC_AbstractGCLiveNarrowing.tla" "MC_AbstractGCLiveNarrowing_templates_live.cfg" \
  fail "Invariant NoFutureTouchFreed is violated"
run_tlc "collapse_fix" "CollapseCompletion.tla" "CollapseCompletion_fix.cfg" \
  pass ""
run_tlc "collapse_bug" "CollapseCompletion.tla" "CollapseCompletion_bug.cfg" \
  fail "Temporal properties were violated"
run_tlc "collapse_slot_bug" "CollapseCompletion.tla" "CollapseCompletion_slot_bug.cfg" \
  fail "Invariant NoSilentSuccessfulDrop is violated"
run_tlc "binding_projection_tracked" "BindingProjection.tla" "MC_BindingProjection_tracked.cfg" \
  pass ""
run_tlc "binding_projection_no_context" "BindingProjection.tla" "MC_BindingProjection_no_context.cfg" \
  pass ""
run_tlc "binding_projection_missing_closure" "BindingProjection.tla" "MC_BindingProjection_missing_closure.cfg" \
  fail "Invariant Inv is violated"
run_tlc "binding_projection_no_projection" "BindingProjection.tla" "MC_BindingProjection_no_projection.cfg" \
  fail "Invariant Inv is violated"
run_tlc "trampoline_fanout_spine_progress_faithful" "TrampolineFanoutSpineProgress.tla" "TrampolineFanoutSpineProgress_faithful.cfg" \
  pass ""
run_tlc "trampoline_fanout_spine_progress_reset" "TrampolineFanoutSpineProgress.tla" "TrampolineFanoutSpineProgress_reset.cfg" \
  fail "Temporal properties were violated"
run_tlc "worker_admission_gate" "MC_WorkerAdmission.tla" "MC_WorkerAdmission_gate.cfg" \
  pass ""
run_tlc "worker_admission_race" "MC_WorkerAdmission.tla" "MC_WorkerAdmission_race.cfg" \
  fail "Invariant NoUnsnapshottedWorkerFreed is violated"
run_tlc "rendezvous_witness_strict" "MC_RendezvousWitness.tla" "MC_RendezvousWitness_strict.cfg" \
  pass ""
run_tlc "rendezvous_witness_weak" "MC_RendezvousWitness.tla" "MC_RendezvousWitness_weak.cfg" \
  fail "Invariant RootCompleteOnSweep is violated"
run_tlc "rendezvous_witness_finisher_stamps" "MC_RendezvousWitness.tla" "MC_RendezvousWitness_finisher_stamps.cfg" \
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
run_tlc "midloop_root_union_all" "MC_MidloopRootUnion.tla" "MC_MidloopRootUnion_all.cfg" \
  pass ""
run_tlc "midloop_root_union_missing_machine" "MC_MidloopRootUnion.tla" "MC_MidloopRootUnion_missing_machine.cfg" \
  fail "Invariant MidloopRootUnionComplete is violated"
run_tlc "midloop_root_union_missing_deferred" "MC_MidloopRootUnion.tla" "MC_MidloopRootUnion_missing_deferred.cfg" \
  fail "Invariant MidloopRootUnionComplete is violated"
run_tlc "midloop_root_union_missing_driver_c" "MC_MidloopRootUnion.tla" "MC_MidloopRootUnion_missing_driver_c.cfg" \
  fail "Invariant MidloopRootUnionComplete is violated"
run_tlc "k_spine_current_work_all" "MC_KSpineCurrentWork.tla" "MC_KSpineCurrentWork_all.cfg" \
  pass ""
run_tlc "k_spine_current_work_missing_current" "MC_KSpineCurrentWork.tla" "MC_KSpineCurrentWork_missing_current.cfg" \
  fail "Invariant NoLiveControlFreed is violated"
run_tlc "vm_nested_locals_all" "MC_VmNestedLocals.tla" "MC_VmNestedLocals_all.cfg" \
  pass ""
run_tlc "vm_nested_locals_missing_pre_eval" "MC_VmNestedLocals.tla" "MC_VmNestedLocals_missing_pre_eval.cfg" \
  fail "Invariant NoLiveVmLocalFreed is violated"
run_tlc "vm_nested_locals_missing_rule_matches" "MC_VmNestedLocals.tla" "MC_VmNestedLocals_missing_rule_matches.cfg" \
  fail "Invariant NoLiveVmLocalFreed is violated"
run_tlc "non_rendezvous_fanout_blocked" "MC_NonRendezvousFanoutGate.tla" "MC_NonRendezvousFanoutGate_fanout_blocked.cfg" \
  pass ""
run_tlc "non_rendezvous_fanout_zero" "MC_NonRendezvousFanoutGate.tla" "MC_NonRendezvousFanoutGate_fanout_zero.cfg" \
  pass ""
run_tlc "non_rendezvous_fanout_missing_gate" "MC_NonRendezvousFanoutGate.tla" "MC_NonRendezvousFanoutGate_missing_gate.cfg" \
  fail "Invariant NoMidloopNonRendezvousUnderFanout is violated"
run_tlc "registry_isolation_all" "MC_RegistryIsolation.tla" "MC_RegistryIsolation_all.cfg" \
  pass ""
run_tlc "registry_isolation_enabled" "MC_RegistryIsolation.tla" "MC_RegistryIsolation_registry.cfg" \
  fail "Invariant NoRegistryInIndex is violated"
run_tlc "rendezvous_quiescence_independence_all" "MC_RendezvousQuiescenceIndependence.tla" "MC_RendezvousQuiescenceIndependence_all.cfg" \
  pass ""
run_tlc "rendezvous_quiescence_independence_global_gate" "MC_RendezvousQuiescenceIndependence.tla" "MC_RendezvousQuiescenceIndependence_global_gate.cfg" \
  fail "Invariant ReadyCanSweep is violated"
run_tlc "poll_edge_contribution_all" "MC_PollEdgeContribution.tla" "MC_PollEdgeContribution_all.cfg" \
  pass ""
run_tlc "poll_edge_contribution_missing_publish" "MC_PollEdgeContribution.tla" "MC_PollEdgeContribution_missing_publish.cfg" \
  fail "Invariant WaitAfterContribution is violated"
run_tlc "frame_env_roots_all" "MC_FrameEnvRoots.tla" "MC_FrameEnvRoots_all.cfg" \
  pass ""
run_tlc "frame_env_roots_missing_inferred" "MC_FrameEnvRoots.tla" "MC_FrameEnvRoots_missing_inferred.cfg" \
  fail "Invariant FrameEnvRootsComplete is violated"
run_tlc "tier_leaf_extra_roots_all" "MC_TierLeafExtraRoots.tla" "MC_TierLeafExtraRoots_all.cfg" \
  pass ""
run_tlc "tier_leaf_extra_roots_missing_vm_dispatch_memo" "MC_TierLeafExtraRoots.tla" "MC_TierLeafExtraRoots_missing_vm_dispatch_memo.cfg" \
  fail "Invariant TierLeafExtraRootsComplete is violated"
run_tlc "tier_leaf_extra_roots_missing_jit_state_cache" "MC_TierLeafExtraRoots.tla" "MC_TierLeafExtraRoots_missing_jit_state_cache.cfg" \
  fail "Invariant TierLeafExtraRootsComplete is violated"
run_tlc "selective_choice_point_roots_all" "MC_SelectiveChoicePointRoots.tla" "MC_SelectiveChoicePointRoots_all.cfg" \
  pass ""
run_tlc "selective_choice_point_roots_missing_vm" "MC_SelectiveChoicePointRoots.tla" "MC_SelectiveChoicePointRoots_missing_vm.cfg" \
  fail "Invariant NoReenterableChoiceFreed is violated"
run_tlc "selective_choice_point_roots_missing_jit" "MC_SelectiveChoicePointRoots.tla" "MC_SelectiveChoicePointRoots_missing_jit.cfg" \
  fail "Invariant NoReenterableChoiceFreed is violated"
run_tlc "selective_choice_point_roots_missing_trampoline" "MC_SelectiveChoicePointRoots.tla" "MC_SelectiveChoicePointRoots_missing_trampoline.cfg" \
  fail "Invariant NoReenterableChoiceFreed is violated"
run_tlc "serializable_continuation_slice_all" "MC_SerializableContinuationSlice.tla" "MC_SerializableContinuationSlice_all.cfg" \
  pass ""
run_tlc "serializable_continuation_slice_missing_child" "MC_SerializableContinuationSlice.tla" "MC_SerializableContinuationSlice_missing_child.cfg" \
  fail "Invariant NoRestoredFutureTouchFreed is violated"
run_tlc "serializable_continuation_slice_missing_kont" "MC_SerializableContinuationSlice.tla" "MC_SerializableContinuationSlice_missing_kont.cfg" \
  fail "Invariant NoRestoredFutureTouchFreed is violated"
run_tlc "index_arena_publication_all" "MC_IndexArenaPublication.tla" "MC_IndexArenaPublication_all.cfg" \
  pass ""
run_tlc "index_arena_publication_segment_publish_before_write" "MC_IndexArenaPublication.tla" "MC_IndexArenaPublication_segment_publish_before_write.cfg" \
  fail "Invariant ReturnedAddrReady is violated"
run_tlc "index_arena_publication_slot_before_segment" "MC_IndexArenaPublication.tla" "MC_IndexArenaPublication_slot_before_segment.cfg" \
  fail "Invariant ReturnedAddrReady is violated"
run_tlc "index_arena_publication_slot_publish_before_write" "MC_IndexArenaPublication.tla" "MC_IndexArenaPublication_slot_publish_before_write.cfg" \
  fail "Invariant ReturnedAddrReady is violated"
run_tlc "index_arena_publication_return_before_publish" "MC_IndexArenaPublication.tla" "MC_IndexArenaPublication_return_before_publish.cfg" \
  fail "Invariant ReturnedAddrReady is violated"
run_tlc "side_arena_publication_all" "MC_SideArenaPublication.tla" "MC_SideArenaPublication_all.cfg" \
  pass ""
run_tlc "side_arena_publication_chunk_before_page" "MC_SideArenaPublication.tla" "MC_SideArenaPublication_chunk_before_page.cfg" \
  fail "Invariant PublishedEntryReady is violated"
run_tlc "side_arena_publication_entry_before_chunk" "MC_SideArenaPublication.tla" "MC_SideArenaPublication_entry_before_chunk.cfg" \
  fail "Invariant PublishedEntryReady is violated"
run_tlc "side_arena_publication_publish_before_write" "MC_SideArenaPublication.tla" "MC_SideArenaPublication_publish_before_write.cfg" \
  fail "Invariant PublishedEntryReady is violated"
run_tlc "inner_column_read_refinement_all" "InnerColumnReadRefinement.tla" "MC_InnerColumnReadRefinement_all.cfg" \
  pass ""
run_tlc "inner_column_read_refinement_no_rewrite" "InnerColumnReadRefinement.tla" "MC_InnerColumnReadRefinement_no_rewrite.cfg" \
  fail "Invariant NoStaleRead is violated"
run_tlc "inner_column_read_refinement_space_memo_column" "InnerColumnReadRefinement.tla" "MC_InnerColumnReadRefinement_space_memo_column.cfg" \
  fail "Invariant NoStaleRead is violated"
run_tlc "side_arena_colocation_colocated" "MC_SideArenaCoLocation.tla" "MC_SideArenaCoLocation_colocated.cfg" \
  pass ""
run_tlc "side_arena_colocation_wrong_segment" "MC_SideArenaCoLocation.tla" "MC_SideArenaCoLocation_wrong_segment.cfg" \
  fail "Invariant NoBadSideRead is violated"
run_tlc "side_arena_colocation_publish_before_side" "MC_SideArenaCoLocation.tla" "MC_SideArenaCoLocation_publish_before_side.cfg" \
  fail "Invariant NoBadSideRead is violated"
run_tlc "concurrent_bump_fresh_only_all" "MC_ConcurrentBumpFreshOnly.tla" "MC_ConcurrentBumpFreshOnly_all.cfg" \
  pass ""
run_tlc "concurrent_bump_fresh_only_concurrent_reuse" "MC_ConcurrentBumpFreshOnly.tla" "MC_ConcurrentBumpFreshOnly_concurrent_reuse.cfg" \
  fail "Invariant ConcurrentNeverReturnsFreeList is violated"
run_tlc "concurrent_bump_fresh_only_nonexclusive_reuse" "MC_ConcurrentBumpFreshOnly.tla" "MC_ConcurrentBumpFreshOnly_nonexclusive_reuse.cfg" \
  fail "Invariant ReuseOnlyExclusive is violated"
run_tlc "side_free_quiescence" "MC_SideFreeQuiescence.tla" "MC_SideFreeQuiescence_quiescent.cfg" \
  pass ""
run_tlc "side_free_midloop_deferred" "MC_SideFreeQuiescence.tla" "MC_SideFreeQuiescence_midloop_deferred.cfg" \
  pass ""
run_tlc "side_free_missing_gate" "MC_SideFreeQuiescence.tla" "MC_SideFreeQuiescence_missing_gate.cfg" \
  fail "Invariant NoDanglingSideUse is violated"
run_tlc "side_free_missing_shadow_clear" "MC_SideFreeQuiescence.tla" "MC_SideFreeQuiescence_missing_shadow_clear.cfg" \
  fail "Invariant NoDanglingSideUse is violated"
run_tlc "side_reclaim_snapshot_safe" "MC_SideReclaimSnapshot.tla" "MC_SideReclaimSnapshot_safe.cfg" \
  pass ""
run_tlc "side_reclaim_snapshot_no_snapshot" "MC_SideReclaimSnapshot.tla" "MC_SideReclaimSnapshot_no_snapshot.cfg" \
  fail "Invariant NoLiveSideFreed is violated"
run_tlc "side_reclaim_snapshot_no_reset_drop" "MC_SideReclaimSnapshot.tla" "MC_SideReclaimSnapshot_no_reset_drop.cfg" \
  fail "Invariant NoLiveSideFreed is violated"
run_tlc "side_reclaim_snapshot_no_free_owner_gate" "MC_SideReclaimSnapshot.tla" "MC_SideReclaimSnapshot_no_free_owner_gate.cfg" \
  fail "Invariant NoLiveSideFreed is violated"
run_tlc "side_reclaim_snapshot_minor_drain" "MC_SideReclaimSnapshot.tla" "MC_SideReclaimSnapshot_minor_drain.cfg" \
  fail "Invariant NoLiveSideFreed is violated"
run_tlc "side_reclaim_snapshot_no_marked_owner_filter" "MC_SideReclaimSnapshot.tla" "MC_SideReclaimSnapshot_no_marked_owner_filter.cfg" \
  fail "Invariant NoLiveSideFreed is violated"
run_tlc "side_reclaim_generation_guard" "MC_SideReclaimGeneration.tla" "MC_SideReclaimGeneration_guard.cfg" \
  pass ""
run_tlc "side_reclaim_generation_no_guard" "MC_SideReclaimGeneration.tla" "MC_SideReclaimGeneration_no_guard.cfg" \
  fail "Invariant NoLiveCellFreed is violated"
# #273: with pending_side_major the rendezvous collector drains side reclaims every
# quiescence (PendingBounded holds); WITHOUT it (pre-266d19d) pending grows unbounded.
run_tlc "rendezvous_side_reclaim_bounded" "MC_RendezvousSideReclaimProgress.tla" "MC_RendezvousSideReclaimProgress_bounded.cfg" \
  pass ""
run_tlc "rendezvous_side_reclaim_unbounded" "MC_RendezvousSideReclaimProgress.tla" "MC_RendezvousSideReclaimProgress_unbounded.cfg" \
  fail "Invariant PendingBounded is violated"
run_tlc "hash_cons_major_safe" "MC_HashConsSweepRetain.tla" "MC_HashConsSweepRetain_major_safe.cfg" \
  pass ""
run_tlc "hash_cons_major_dead_validated" "MC_HashConsSweepRetain.tla" "MC_HashConsSweepRetain_major_dead_validated.cfg" \
  pass ""
run_tlc "hash_cons_major_missing_side_validated" "MC_HashConsSweepRetain.tla" "MC_HashConsSweepRetain_major_missing_side_validated.cfg" \
  pass ""
run_tlc "hash_cons_major_dead_retained" "MC_HashConsSweepRetain.tla" "MC_HashConsSweepRetain_major_dead_retained.cfg" \
  fail "Invariant NoReturnedFreed is violated"
run_tlc "hash_cons_minor_old_retained" "MC_HashConsSweepRetain.tla" "MC_HashConsSweepRetain_minor_old_retained.cfg" \
  pass ""
run_tlc "hash_cons_minor_dead_young_validated" "MC_HashConsSweepRetain.tla" "MC_HashConsSweepRetain_minor_dead_young_validated.cfg" \
  pass ""
run_tlc "hash_cons_minor_dead_young_retained" "MC_HashConsSweepRetain.tla" "MC_HashConsSweepRetain_minor_dead_young_retained.cfg" \
  fail "Invariant NoReturnedFreed is violated"
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
run_tlc "rholang_batch_completion_guarded" "RholangBatchCompletion.tla" "RholangBatchCompletion_guarded.cfg" \
  pass ""
run_tlc "rholang_batch_completion_panic_bug" "RholangBatchCompletion.tla" "RholangBatchCompletion_panic_bug.cfg" \
  fail "Temporal properties were violated"
run_tlc "rholang_batch_completion_slot_bug" "RholangBatchCompletion.tla" "RholangBatchCompletion_slot_bug.cfg" \
  fail "Invariant NoSilentBatchSuccess is violated"
run_tlc "scheduler_gc_boundary_all" "MC_SchedulerGcBoundary.tla" "MC_SchedulerGcBoundary_all.cfg" \
  pass ""
run_tlc "scheduler_gc_boundary_missing_worker" "MC_SchedulerGcBoundary.tla" "MC_SchedulerGcBoundary_missing_worker.cfg" \
  fail "Invariant SchedulerBoundaryComplete is violated"
run_tlc "scheduler_gc_boundary_missing_dispatch" "MC_SchedulerGcBoundary.tla" "MC_SchedulerGcBoundary_missing_dispatch.cfg" \
  fail "Invariant SchedulerBoundaryComplete is violated"
run_tlc "scheduler_gc_boundary_missing_batch" "MC_SchedulerGcBoundary.tla" "MC_SchedulerGcBoundary_missing_batch.cfg" \
  fail "Invariant SchedulerBoundaryComplete is violated"
run_tlc "scheduler_gc_boundary_admission_open" "MC_SchedulerGcBoundary.tla" "MC_SchedulerGcBoundary_admission_open.cfg" \
  fail "Invariant SchedulerBoundaryComplete is violated"
run_tlc "scheduler_spawn_latch_all" "SchedulerSpawnLatch.tla" "MC_SchedulerSpawnLatch_all.cfg" \
  pass ""
run_tlc "scheduler_spawn_latch_spawn_before_latch" "SchedulerSpawnLatch.tla" "MC_SchedulerSpawnLatch_spawn_before_latch.cfg" \
  fail "Invariant NoWorkerWithMidloopGateOpen is violated"
run_tlc "scheduler_spawn_latch_missing_latch" "SchedulerSpawnLatch.tla" "MC_SchedulerSpawnLatch_missing_latch.cfg" \
  fail "Invariant NoWorkerWithMidloopGateOpen is violated"
run_tlc "dedicated_handoff_skip" "MC_DedicatedHandoff.tla" "MC_DedicatedHandoff_skip.cfg" \
  pass ""
run_tlc "dedicated_handoff_empty_inline" "MC_DedicatedHandoff.tla" "MC_DedicatedHandoff_empty_inline.cfg" \
  fail "Invariant NoInlineWithoutRoots is violated"
run_tlc "dedicated_handoff_no_reply" "MC_DedicatedHandoff.tla" "MC_DedicatedHandoff_no_reply.cfg" \
  fail "Invariant ConsumedRequestGetsReplyAttempt is violated"
run_tlc "dedicated_single_regime_all_gated" "MC_DedicatedSingleRegime.tla" "MC_DedicatedSingleRegime_all_gated.cfg" \
  pass ""
run_tlc "dedicated_single_regime_default_ungated" "MC_DedicatedSingleRegime.tla" "MC_DedicatedSingleRegime_default_ungated.cfg" \
  fail "Invariant NoDriverlessRequest is violated"
run_tlc "dedicated_single_regime_session_ungated" "MC_DedicatedSingleRegime.tla" "MC_DedicatedSingleRegime_session_ungated.cfg" \
  fail "Invariant NoDriverlessRequest is violated"
run_tlc "dedicated_single_regime_parallel_ungated" "MC_DedicatedSingleRegime.tla" "MC_DedicatedSingleRegime_parallel_ungated.cfg" \
  fail "Invariant NoDriverlessRequest is violated"
run_tlc "dedicated_single_regime_cron_ungated" "MC_DedicatedSingleRegime.tla" "MC_DedicatedSingleRegime_cron_ungated.cfg" \
  fail "Invariant NoDriverlessRequest is violated"
run_tlc "depth_zero_safepoint_guarded" "MC_DepthZeroSafepoint.tla" "MC_DepthZeroSafepoint_guarded.cfg" \
  pass ""
run_tlc "depth_zero_safepoint_no_guard" "MC_DepthZeroSafepoint.tla" "MC_DepthZeroSafepoint_no_guard.cfg" \
  fail "Invariant DepthZeroDoesNotPark is violated"
run_tlc "concurrent_trigger_backstop_sent" "MC_ConcurrentTriggerBackstop.tla" "MC_ConcurrentTriggerBackstop_sent.cfg" \
  pass ""
run_tlc "concurrent_trigger_backstop_spawn_none" "MC_ConcurrentTriggerBackstop.tla" "MC_ConcurrentTriggerBackstop_spawn_none.cfg" \
  pass ""
run_tlc "concurrent_trigger_backstop_send_fail" "MC_ConcurrentTriggerBackstop.tla" "MC_ConcurrentTriggerBackstop_send_fail.cfg" \
  pass ""
run_tlc "concurrent_trigger_backstop_no_spawn" "MC_ConcurrentTriggerBackstop.tla" "MC_ConcurrentTriggerBackstop_no_spawn_backstop.cfg" \
  fail "Invariant FailedTriggerClearsRequest is violated"
run_tlc "concurrent_trigger_backstop_no_send" "MC_ConcurrentTriggerBackstop.tla" "MC_ConcurrentTriggerBackstop_no_send_backstop.cfg" \
  fail "Invariant FailedTriggerClearsRequest is violated"
run_tlc "operator_cache_epoch_checked" "MC_OperatorCacheEpoch.tla" "MC_OperatorCacheEpoch_checked.cfg" \
  pass ""
run_tlc "operator_cache_epoch_unchecked" "MC_OperatorCacheEpoch.tla" "MC_OperatorCacheEpoch_unchecked.cfg" \
  fail "Invariant NoStaleOperatorCacheHit is violated"
run_tlc "epoch_protected_caches_all" "MC_EpochProtectedCaches.tla" "MC_EpochProtectedCaches_all.cfg" \
  pass ""
run_tlc "epoch_protected_caches_missing_value_hash" "MC_EpochProtectedCaches.tla" "MC_EpochProtectedCaches_missing_value_hash.cfg" \
  fail "Invariant NoStaleAddrCacheHit is violated"
run_tlc "epoch_protected_caches_missing_mork" "MC_EpochProtectedCaches.tla" "MC_EpochProtectedCaches_missing_mork.cfg" \
  fail "Invariant NoStaleAddrCacheHit is violated"
run_tlc "epoch_protected_caches_missing_hash_cons" "MC_EpochProtectedCaches.tla" "MC_EpochProtectedCaches_missing_hash_cons.cfg" \
  fail "Invariant NoStaleAddrCacheHit is violated"
run_tlc "epoch_protected_caches_missing_eval" "MC_EpochProtectedCaches.tla" "MC_EpochProtectedCaches_missing_eval.cfg" \
  fail "Invariant NoStaleAddrCacheHit is violated"
run_tlc "epoch_protected_caches_missing_match" "MC_EpochProtectedCaches.tla" "MC_EpochProtectedCaches_missing_match.cfg" \
  fail "Invariant NoStaleAddrCacheHit is violated"
run_tlc "epoch_protected_caches_missing_operator" "MC_EpochProtectedCaches.tla" "MC_EpochProtectedCaches_missing_operator.cfg" \
  fail "Invariant NoStaleAddrCacheHit is violated"
run_tlc "epoch_protected_caches_missing_inner_shadow" "MC_EpochProtectedCaches.tla" "MC_EpochProtectedCaches_missing_inner_shadow.cfg" \
  fail "Invariant NoStaleAddrCacheHit is violated"
run_tlc "write_once_anchors_all" "MC_WriteOnceAnchors.tla" "MC_WriteOnceAnchors_all.cfg" \
  pass ""
run_tlc "write_once_anchors_missing_if" "MC_WriteOnceAnchors.tla" "MC_WriteOnceAnchors_missing_if.cfg" \
  fail "Invariant LiveAnchorsScanned is violated"
run_tlc "write_once_anchors_delete" "MC_WriteOnceAnchors.tla" "MC_WriteOnceAnchors_delete.cfg" \
  fail "Invariant NoAnchorDeleted is violated"
run_tlc "space_registry_barriers_all" "MC_SpaceRegistryBarriers.tla" "MC_SpaceRegistryBarriers_all.cfg" \
  pass ""
run_tlc "space_registry_barriers_missing_scan" "MC_SpaceRegistryBarriers.tla" "MC_SpaceRegistryBarriers_missing_scan.cfg" \
  fail "Invariant NoSpaceRegistryValueFreed is violated"
run_tlc "space_registry_barriers_missing_remove" "MC_SpaceRegistryBarriers.tla" "MC_SpaceRegistryBarriers_missing_remove.cfg" \
  fail "Invariant NoSpaceRegistryValueFreed is violated"
run_tlc "space_registry_barriers_missing_clear" "MC_SpaceRegistryBarriers.tla" "MC_SpaceRegistryBarriers_missing_clear.cfg" \
  fail "Invariant NoSpaceRegistryValueFreed is violated"
run_tlc "tiered_cache_barriers_all" "MC_TieredCacheBarriers.tla" "MC_TieredCacheBarriers_all.cfg" \
  pass ""
run_tlc "tiered_cache_barriers_missing_scan_compiled" "MC_TieredCacheBarriers.tla" "MC_TieredCacheBarriers_missing_scan_compiled.cfg" \
  fail "Invariant NoTieredCacheValueFreed is violated"
run_tlc "tiered_cache_barriers_missing_cancel" "MC_TieredCacheBarriers.tla" "MC_TieredCacheBarriers_missing_cancel.cfg" \
  fail "Invariant NoTieredCacheValueFreed is violated"
run_tlc "tiered_cache_barriers_missing_clear_compiled" "MC_TieredCacheBarriers.tla" "MC_TieredCacheBarriers_missing_clear_compiled.cfg" \
  fail "Invariant NoTieredCacheValueFreed is violated"
run_tlc "tiered_cache_barriers_unchecked_guard_drop" "MC_TieredCacheBarriers.tla" "MC_TieredCacheBarriers_unchecked_guard_drop.cfg" \
  fail "Invariant NoTieredCacheValueFreed is violated"
run_tlc "thread_local_tables_barriers_all" "MC_ThreadLocalTablesBarriers.tla" "MC_ThreadLocalTablesBarriers_all.cfg" \
  pass ""
run_tlc "thread_local_tables_barriers_missing_scan_thunk" "MC_ThreadLocalTablesBarriers.tla" "MC_ThreadLocalTablesBarriers_missing_scan_thunk.cfg" \
  fail "Invariant NoThreadLocalTableValueFreed is violated"
run_tlc "thread_local_tables_barriers_missing_subgoal_stale" "MC_ThreadLocalTablesBarriers.tla" "MC_ThreadLocalTablesBarriers_missing_subgoal_stale.cfg" \
  fail "Invariant NoThreadLocalTableValueFreed is violated"
run_tlc "thread_local_tables_barriers_missing_thunk_clear" "MC_ThreadLocalTablesBarriers.tla" "MC_ThreadLocalTablesBarriers_missing_thunk_clear.cfg" \
  fail "Invariant NoThreadLocalTableValueFreed is violated"
run_tlc "thread_local_tables_barriers_missing_thunk_replace" "MC_ThreadLocalTablesBarriers.tla" "MC_ThreadLocalTablesBarriers_missing_thunk_replace.cfg" \
  fail "Invariant NoThreadLocalTableValueFreed is violated"
run_tlc "eval_tables_registered_roots_all" "MC_EvalTablesRegisteredRoots.tla" "MC_EvalTablesRegisteredRoots_all.cfg" \
  pass ""
run_tlc "eval_tables_missing_eval_memo" "MC_EvalTablesRegisteredRoots.tla" "MC_EvalTablesRegisteredRoots_missing_eval_memo.cfg" \
  fail "Invariant NoEvalTableValueFreed is violated"
run_tlc "eval_tables_missing_match_result" "MC_EvalTablesRegisteredRoots.tla" "MC_EvalTablesRegisteredRoots_missing_match_result.cfg" \
  fail "Invariant NoEvalTableValueFreed is violated"
run_tlc "started_cycle_gate_started" "MC_StartedCycleGate.tla" "MC_StartedCycleGate_started.cfg" \
  pass ""
run_tlc "started_cycle_gate_gen" "MC_StartedCycleGate.tla" "MC_StartedCycleGate_gen.cfg" \
  fail "Invariant NoPhantomRepark is violated"
run_tlc "generation_resume_gen" "MC_GenerationResume.tla" "MC_GenerationResume_gen.cfg" \
  pass ""
run_tlc "generation_resume_boolean" "MC_GenerationResume.tla" "MC_GenerationResume_boolean.cfg" \
  fail "Invariant EndedCycleCanResume is violated"
run_tlc "generation_resume_no_bump" "MC_GenerationResume.tla" "MC_GenerationResume_no_bump.cfg" \
  fail "Invariant EndedCycleCanResume is violated"
run_tlc "rendezvous_progress_all" "MC_RendezvousProgress.tla" "MC_RendezvousProgress_all.cfg" \
  pass ""
run_tlc "rendezvous_progress_one_participant" "MC_RendezvousProgress.tla" "MC_RendezvousProgress_one_participant.cfg" \
  pass ""
run_tlc "rendezvous_progress_missing_contribution" "MC_RendezvousProgress.tla" "MC_RendezvousProgress_missing_contribution.cfg" \
  fail "Temporal properties were violated"
run_tlc "rendezvous_progress_panic_no_cleanup" "MC_RendezvousProgress.tla" "MC_RendezvousProgress_panic_no_cleanup.cfg" \
  fail "Temporal properties were violated"
run_tlc "rendezvous_progress_no_gen_bump" "MC_RendezvousProgress.tla" "MC_RendezvousProgress_no_gen_bump.cfg" \
  fail "Temporal properties were violated"
run_tlc "rendezvous_progress_boolean_resume" "MC_RendezvousProgress.tla" "MC_RendezvousProgress_boolean_resume.cfg" \
  fail "Temporal properties were violated"
run_tlc "rendezvous_progress_no_resume_notify" "MC_RendezvousProgress.tla" "MC_RendezvousProgress_no_resume_notify.cfg" \
  fail "Temporal properties were violated"
# E5: SATB DOUBLE-rendezvous straddle liveness (PostCycleEvaluatorProgress). The fix
# (started-gate + B-closure + reopen-notify) closes every posted cycle; each missing
# mechanism strands the parent. Safety NoSweepWhileUnpublished holds in ALL 4.
run_tlc "post_cycle_evaluator_progress_fix" "MC_PostCycleEvaluatorProgress.tla" "MC_PostCycleEvaluatorProgress_fix.cfg" \
  pass ""
run_tlc "post_cycle_evaluator_progress_reopen_no_notify" "MC_PostCycleEvaluatorProgress.tla" "MC_PostCycleEvaluatorProgress_reopen_no_notify.cfg" \
  fail "Temporal properties were violated"
run_tlc "post_cycle_evaluator_progress_no_bclosure" "MC_PostCycleEvaluatorProgress.tla" "MC_PostCycleEvaluatorProgress_no_bclosure.cfg" \
  fail "Temporal properties were violated"
run_tlc "post_cycle_evaluator_progress_gen_gate" "MC_PostCycleEvaluatorProgress.tla" "MC_PostCycleEvaluatorProgress_gen_gate.cfg" \
  fail "Temporal properties were violated"
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
run_tlc "nursery_backpressure_all" "MC_NurseryBackpressure.tla" "MC_NurseryBackpressure_all.cfg" \
  pass ""
run_tlc "nursery_backpressure_no_signal" "MC_NurseryBackpressure.tla" "MC_NurseryBackpressure_no_signal.cfg" \
  fail "Invariant NurseryOpenSignalsPending is violated"
run_tlc "nursery_backpressure_no_fold" "MC_NurseryBackpressure.tla" "MC_NurseryBackpressure_no_fold.cfg" \
  fail "Invariant OpenRequestsMinor is violated"
run_tlc "nursery_backpressure_no_clear" "MC_NurseryBackpressure.tla" "MC_NurseryBackpressure_no_clear.cfg" \
  fail "Invariant PromoteRelaxesMinorTrigger is violated"
run_tlc "young_allocation_odometer_all" "MC_YoungAllocationOdometer.tla" "MC_YoungAllocationOdometer_all.cfg" \
  pass ""
run_tlc "young_allocation_odometer_no_reuse" "MC_YoungAllocationOdometer.tla" "MC_YoungAllocationOdometer_no_reuse.cfg" \
  fail "Invariant ReuseCountsYoungAllocation is violated"
run_tlc "young_allocation_odometer_no_bump" "MC_YoungAllocationOdometer.tla" "MC_YoungAllocationOdometer_no_bump.cfg" \
  fail "Invariant BumpCountsYoungAllocation is violated"
run_tlc "young_allocation_odometer_no_reset" "MC_YoungAllocationOdometer.tla" "MC_YoungAllocationOdometer_no_reset.cfg" \
  fail "Invariant PromotionResetsOdometer is violated"
run_tlc "major_minor_scheduler_all" "MC_MajorMinorScheduler.tla" "MC_MajorMinorScheduler_all.cfg" \
  pass ""
run_tlc "major_minor_scheduler_no_level" "MC_MajorMinorScheduler.tla" "MC_MajorMinorScheduler_no_level.cfg" \
  fail "Invariant DeferredMajorRequiresLevel3 is violated"
run_tlc "major_minor_scheduler_no_cap" "MC_MajorMinorScheduler.tla" "MC_MajorMinorScheduler_no_cap.cfg" \
  fail "Invariant CapMajorNotDeferred is violated"
run_tlc "major_minor_scheduler_no_cadence" "MC_MajorMinorScheduler.tla" "MC_MajorMinorScheduler_no_cadence.cfg" \
  fail "Invariant CadenceMajorNotDeferred is violated"
run_tlc "cap_floor_anti_thrash_all" "MC_CapFloorAntiThrash.tla" "MC_CapFloorAntiThrash_all.cfg" \
  pass ""
run_tlc "cap_floor_anti_thrash_no_raise" "MC_CapFloorAntiThrash.tla" "MC_CapFloorAntiThrash_no_raise.cfg" \
  fail "Invariant FutileCapMajorDoesNotRefire is violated"
run_tlc "cap_floor_anti_thrash_no_clear" "MC_CapFloorAntiThrash.tla" "MC_CapFloorAntiThrash_no_clear.cfg" \
  fail "Invariant ReleaseClearsFloor is violated"
run_tlc "major_watermark_rearm_all" "MC_MajorWatermarkRearm.tla" "MC_MajorWatermarkRearm_all.cfg" \
  pass ""
run_tlc "major_watermark_rearm_no_rearm" "MC_MajorWatermarkRearm.tla" "MC_MajorWatermarkRearm_no_rearm.cfg" \
  fail "Invariant ImmediateOldLiveDoesNotRefire is violated"
run_tlc "major_watermark_rearm_no_growth" "MC_MajorWatermarkRearm.tla" "MC_MajorWatermarkRearm_no_growth.cfg" \
  fail "Invariant FutureRefireRequiresDoubledGrowth is violated"
run_tlc "satb_trigger_suppression_gate" "MC_SATBTriggerSuppression.tla" "MC_SATBTriggerSuppression_gate.cfg" \
  pass ""
run_tlc "satb_trigger_suppression_no_gate" "MC_SATBTriggerSuppression.tla" "MC_SATBTriggerSuppression_no_gate.cfg" \
  fail "Invariant ActiveSATBSuppressesWatermarkTrigger is violated"
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
run_tlc "satb_final_remark_premarked_revisit" "MC_SATBFinalRemarkPremarked.tla" "MC_SATBFinalRemarkPremarked_revisit.cfg" \
  pass ""
run_tlc "satb_final_remark_premarked_new_only" "MC_SATBFinalRemarkPremarked.tla" "MC_SATBFinalRemarkPremarked_new_only.cfg" \
  fail "Invariant NoReachableChildFreed is violated"
run_tlc "satb_final_sweep_result_checked" "MC_SATBFinalSweepResult.tla" "MC_SATBFinalSweepResult_checked.cfg" \
  pass ""
run_tlc "satb_final_sweep_result_unchecked" "MC_SATBFinalSweepResult.tla" "MC_SATBFinalSweepResult_unchecked.cfg" \
  fail "Invariant FinalSweepHandled is violated"
run_tlc "satb_young_sweep_full" "MC_SATBYoungSweepStaleOldMark.tla" "MC_SATBYoungSweepStaleOldMark_full.cfg" \
  pass ""
run_tlc "satb_young_sweep_young_only" "MC_SATBYoungSweepStaleOldMark.tla" "MC_SATBYoungSweepStaleOldMark_young_only.cfg" \
  fail "Invariant NoStaleOldMark is violated"

# Bug #309 phantom-future park: the gated protocol is live + phantom-bump-free;
# the ungated protocol deadlocks at the phantom-parked state (the captured wedge).
run_tlc "parked_phantom_gated" "MC_ParkedPhantomCycleGate.tla" "MC_ParkedPhantomCycleGate_gated.cfg" \
  pass ""
run_tlc "parked_phantom_ungated" "MC_ParkedPhantomCycleGate.tla" "MC_ParkedPhantomCycleGate_phantom.cfg" \
  fail "Deadlock reached"
# Bug #309 (second mechanism): coalesced requests must not starve the witness
# wait — the driver re-asserts the request at every rendezvous open.
run_tlc "request_reassert_at_open" "MC_RequestReassertAtOpen.tla" "MC_RequestReassertAtOpen_reassert.cfg" \
  pass ""
run_tlc "request_lost_at_open" "MC_RequestReassertAtOpen.tla" "MC_RequestReassertAtOpen_lost.cfg" \
  fail "Deadlock reached"
run_tlc "satb_abort_fallback" "MC_SATBAbortFallback.tla" "MC_SATBAbortFallback_stw.cfg" \
  pass ""
run_tlc "satb_abort_no_fallback" "MC_SATBAbortFallback.tla" "MC_SATBAbortFallback_none.cfg" \
  fail "Invariant AbortHasBackstop is violated"
run_tlc "satb_abort_no_request" "MC_SATBAbortFallback.tla" "MC_SATBAbortFallback_no_request.cfg" \
  fail "Invariant FallbackSTWRequested is violated"

echo "CESK GC formal checks passed"
