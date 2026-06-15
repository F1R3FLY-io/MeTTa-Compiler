(** End-to-end threading/scheduler/cron/GC interleaving envelope.

    This proof composes the audit obligations that were previously checked in
    separate files: scheduler reordering, effect-conflict exclusion, direct
    fanout maximality for independent work, active-worker GC rooting, closed
    worker admission during a root snapshot, eval-worker spawn latching,
    FANOUT parked-worker resume and completion-drop progress, and recurring
    cron in_flight claims plus stop-before-redispatch.
*)

From Stdlib Require Import Bool.Bool.
From Stdlib Require Import Arith Lia.
From Stdlib Require Import ZArith.
Require Import CronRecurringDispatch.
Require Import CronStartupDelivery.
Require Import CESKCollectorSafety.
Require Import CollapseFanoutAdmissionCompleteness.
Require Import DedicatedHandoff.
Require Import E1DefaultConcurrentFlip.
Require Import E1SatbStwDriverProgress.
Require Import GcDriverChannelProtocol.
Require Import SchedulerActiveFanoutGate.
Require Import SchedulerClassificationLookup.
Require Import SchedulerDirectFanoutWavefrontRefinement.
Require Import SchedulerDynamicEvalGate.
Require Import SchedulerEffectConflictCompleteness.
Require Import SchedulerFanoutAdmissionCompleteness.
Require Import SchedulerFanoutProgress.
Require Import SchedulerGcBoundary.
Require Import SchedulerPriorityFairness.
Require Import SchedulerSpawnLatch.
Require Import SchedulerTransducerParallelism.
Require Import SchedulerWavefrontParallelism.
Require Import WorkPoolLifecycle.
Require Import WorkPoolOverflowCap.
Require Import WorkPoolPanicIsolation.
Require Import WorkPoolStartupDrain.

Import MeTTaTron_GC_CronRecurringDispatch.
Import MeTTaTron_GC_CronStartupDelivery.
Import MeTTaTron_GC_SchedulerActiveFanoutGate.
Import MeTTaTron_GC_SchedulerDynamicEvalGate.
Import MeTTaTron_GC_SchedulerFanoutProgress.
Import MeTTaTron_GC_SchedulerGcBoundary.
Import MeTTaTron_GC_SchedulerPriorityFairness.
Import MeTTaTron_GC_SchedulerSpawnLatch.
Import MeTTaTron_GC_SchedulerTransducerParallelism.
Import MeTTaTron_GC_WorkPoolLifecycle.
Import MeTTaTron_GC_WorkPoolPanicIsolation.
Import MeTTaTron_GC_WorkPoolStartupDrain.

Open Scope nat_scope.

Module MeTTaTron_GC_ThreadingEndToEndInterleaving.

Module DirectRefinement :=
  MeTTaTron_GC_SchedulerDirectFanoutWavefrontRefinement.
Module EffectCompleteness :=
  MeTTaTron_GC_SchedulerEffectConflictCompleteness.
Module FanoutAdmission :=
  MeTTaTron_GC_SchedulerFanoutAdmissionCompleteness.
Module CollapseAdmission :=
  MeTTaTron_GC_CollapseFanoutAdmissionCompleteness.
Module Classification :=
  MeTTaTron_GC_SchedulerClassificationLookup.
Module Collector :=
  MeTTaTron_GC_CESKCollectorSafety.
Module Dedicated :=
  MeTTaTron_GC_DedicatedHandoff.
Module DriverChannel :=
  MeTTaTron_GC_GcDriverChannelProtocol.
Module E1Default :=
  MeTTaTron_GC_E1DefaultConcurrentFlip.
Module E1Driver :=
  MeTTaTron_GC_E1SatbStwDriverProgress.
Module Wavefront :=
  MeTTaTron_GC_SchedulerWavefrontParallelism.

Section EndToEndModel.
  Inductive Task : Type :=
  | Producer : Task
  | Consumer : Task.

  Inductive BoundaryAddr : Type :=
  | BoundaryActiveWorker : BoundaryAddr
  | BoundaryDispatchFanout : BoundaryAddr
  | BoundaryBatchHandoff : BoundaryAddr
  | BoundaryNewAdmission : BoundaryAddr.

  Record Workload : Type := {
    has_dependency : bool;
    dependency_edge_encoded : bool;
    has_effect_conflict : bool;
    effect_conflict_edge_encoded : bool;
    uses_direct_fanout : bool
  }.

  Record ClassificationLookupConfig : Type := {
    classification_start : nat;
    classification_count : nat;
    classification_later_count : nat;
    classification_shift_later_start : bool
  }.

  Definition classification_later_start
      (c : ClassificationLookupConfig)
      : nat :=
    if classification_shift_later_start c
    then S (classification_start c + classification_count c)
    else classification_start c + classification_count c.

  Definition classification_lookup_safe
      (c : ClassificationLookupConfig)
      : Prop :=
    Classification.disjoint
      (classification_start c)
      (S (classification_count c))
      (classification_later_start c)
      (classification_later_count c).

  Definition complete_classification_lookup : ClassificationLookupConfig :=
    {| classification_start := 0;
       classification_count := 1;
       classification_later_count := 1;
       classification_shift_later_start := true |}.

  Definition no_shift_classification_lookup : ClassificationLookupConfig :=
    {| classification_start := 0;
       classification_count := 1;
       classification_later_count := 1;
       classification_shift_later_start := false |}.

  Theorem shifted_classification_lookup_range_disjoint :
    forall start count later_count,
      classification_lookup_safe
        {| classification_start := start;
           classification_count := count;
           classification_later_count := later_count;
           classification_shift_later_start := true |}.
  Proof.
    intros start count later_count.
    unfold classification_lookup_safe, classification_later_start.
    simpl.
    apply Classification.shifted_adjacent_later_range_is_disjoint.
  Qed.

  Theorem complete_classification_lookup_safe :
    classification_lookup_safe complete_classification_lookup.
  Proof.
    apply shifted_classification_lookup_range_disjoint.
  Qed.

  Theorem no_shift_classification_lookup_exposes_overlap :
    ~ classification_lookup_safe no_shift_classification_lookup.
  Proof.
    intros Hdisjoint.
    unfold classification_lookup_safe, no_shift_classification_lookup,
      classification_later_start in Hdisjoint.
    simpl in Hdisjoint.
    unfold Classification.disjoint, Classification.in_range in Hdisjoint.
    specialize (Hdisjoint 1).
    apply Hdisjoint; lia.
  Qed.

  Inductive E1LegacyProducer : Type :=
  | E1DefaultSafepoint : E1LegacyProducer
  | E1SessionSafepoint : E1LegacyProducer
  | E1ParallelSafepoint : E1LegacyProducer
  | E1CronAsync : E1LegacyProducer.

  Record E1DefaultFlipConfig : Type := {
    e1_index_mode : bool;
    e1_dedicated : bool;
    e1_legacy_default_gated : bool;
    e1_legacy_session_gated : bool;
    e1_legacy_parallel_gated : bool;
    e1_legacy_cron_gated : bool;
    e1_fanout_watermark : bool;
    e1_trigger_sent : bool;
    e1_trigger_failed : bool;
    e1_driver_posted : bool;
    e1_cycle_closed : bool;
    e1_generation_advanced : bool;
    e1_request_cleared : bool;
    e1_workers_resumed : bool
  }.

  Definition e1_legacy_request
      (c : E1DefaultFlipConfig)
      (p : E1LegacyProducer)
      : Prop :=
    match p with
    | E1DefaultSafepoint => e1_legacy_default_gated c = false
    | E1SessionSafepoint => e1_legacy_session_gated c = false
    | E1ParallelSafepoint => e1_legacy_parallel_gated c = false
    | E1CronAsync => e1_legacy_cron_gated c = false
    end.

  Definition e1_default_dedicated_follows_index
      (c : E1DefaultFlipConfig)
      : Prop :=
    e1_index_mode c = true -> e1_dedicated c = true.

  Definition e1_legacy_requests_suppressed
      (c : E1DefaultFlipConfig)
      : Prop :=
    e1_dedicated c = true ->
    forall p, ~ e1_legacy_request c p.

  Definition e1_fanout_trigger_total
      (c : E1DefaultFlipConfig)
      : Prop :=
    e1_dedicated c = true ->
    e1_fanout_watermark c = true ->
    e1_trigger_sent c = true \/ e1_trigger_failed c = true.

  Definition e1_successful_trigger_posts_driver
      (c : E1DefaultFlipConfig)
      : Prop :=
    e1_trigger_sent c = true -> e1_driver_posted c = true.

  Definition e1_failed_trigger_backstopped
      (c : E1DefaultFlipConfig)
      : Prop :=
    e1_trigger_failed c = true ->
    e1_request_cleared c = true /\ e1_workers_resumed c = true.

  Definition e1_posted_driver_closes_cycle
      (c : E1DefaultFlipConfig)
      : Prop :=
    e1_driver_posted c = true -> e1_cycle_closed c = true.

  Definition e1_closed_cycle_releases_workers
      (c : E1DefaultFlipConfig)
      : Prop :=
    e1_cycle_closed c = true ->
    e1_generation_advanced c = true /\
    e1_request_cleared c = true /\
    e1_workers_resumed c = true.

  Definition e1_default_flip_safe
      (c : E1DefaultFlipConfig)
      : Prop :=
    e1_default_dedicated_follows_index c /\
    e1_legacy_requests_suppressed c /\
    e1_fanout_trigger_total c /\
    e1_successful_trigger_posts_driver c /\
    e1_failed_trigger_backstopped c /\
    e1_posted_driver_closes_cycle c /\
    e1_closed_cycle_releases_workers c.

  Definition complete_e1_default_flip : E1DefaultFlipConfig :=
    {| e1_index_mode := true;
       e1_dedicated := true;
       e1_legacy_default_gated := true;
       e1_legacy_session_gated := true;
       e1_legacy_parallel_gated := true;
       e1_legacy_cron_gated := true;
       e1_fanout_watermark := true;
       e1_trigger_sent := true;
       e1_trigger_failed := false;
       e1_driver_posted := true;
       e1_cycle_closed := true;
       e1_generation_advanced := true;
       e1_request_cleared := true;
       e1_workers_resumed := true |}.

  Definition legacy_default_ungated_e1_default_flip : E1DefaultFlipConfig :=
    {| e1_index_mode := true;
       e1_dedicated := true;
       e1_legacy_default_gated := false;
       e1_legacy_session_gated := true;
       e1_legacy_parallel_gated := true;
       e1_legacy_cron_gated := true;
       e1_fanout_watermark := true;
       e1_trigger_sent := true;
       e1_trigger_failed := false;
       e1_driver_posted := true;
       e1_cycle_closed := true;
       e1_generation_advanced := true;
       e1_request_cleared := true;
       e1_workers_resumed := true |}.

  Definition trigger_failure_missing_backstop_e1_default_flip
      : E1DefaultFlipConfig :=
    {| e1_index_mode := true;
       e1_dedicated := true;
       e1_legacy_default_gated := true;
       e1_legacy_session_gated := true;
       e1_legacy_parallel_gated := true;
       e1_legacy_cron_gated := true;
       e1_fanout_watermark := true;
       e1_trigger_sent := false;
       e1_trigger_failed := true;
       e1_driver_posted := false;
       e1_cycle_closed := false;
       e1_generation_advanced := false;
       e1_request_cleared := false;
       e1_workers_resumed := false |}.

  Theorem complete_e1_default_flip_safe :
    e1_default_flip_safe complete_e1_default_flip.
  Proof.
    unfold e1_default_flip_safe, e1_default_dedicated_follows_index,
      e1_legacy_requests_suppressed, e1_fanout_trigger_total,
      e1_successful_trigger_posts_driver, e1_failed_trigger_backstopped,
      e1_posted_driver_closes_cycle, e1_closed_cycle_releases_workers,
      complete_e1_default_flip, e1_legacy_request.
    cbn.
    repeat split; intros; try (destruct p);
      try firstorder; try reflexivity; try discriminate; try contradiction.
  Qed.

  Theorem e1_default_flip_safe_excludes_driverless_or_stuck_request :
    forall c,
      e1_default_flip_safe c ->
      e1_index_mode c = true ->
      ~ ((exists p, e1_legacy_request c p) \/
         (e1_trigger_sent c = true /\ ~ e1_driver_posted c = true) \/
         (e1_trigger_failed c = true /\ ~ e1_request_cleared c = true)).
  Proof.
    intros c Hsafe Hindex Hbad.
    unfold e1_default_flip_safe in Hsafe.
    destruct Hsafe as
      [Hdefault [Hlegacy [_ [Hposts [Hfailed _]]]]].
    eapply
      (E1Default.default_flip_no_driverless_request_or_stuck_workers
        E1LegacyProducer
        (e1_index_mode c = true)
        (e1_dedicated c = true)
        (e1_trigger_sent c = true)
        (e1_trigger_failed c = true)
        (e1_driver_posted c = true)
        (e1_request_cleared c = true)
        (e1_workers_resumed c = true)
        (e1_legacy_request c)).
    - exact Hdefault.
    - exact Hlegacy.
    - exact Hposts.
    - exact Hfailed.
    - exact Hindex.
    - exact Hbad.
  Qed.

  Theorem e1_default_flip_safe_implies_trigger_progress :
    forall c,
      e1_default_flip_safe c ->
      e1_index_mode c = true ->
      e1_fanout_watermark c = true ->
      (e1_cycle_closed c = true /\
       e1_generation_advanced c = true /\
       e1_request_cleared c = true /\
       e1_workers_resumed c = true) \/
      (e1_request_cleared c = true /\ e1_workers_resumed c = true).
  Proof.
    intros c Hsafe Hindex Hwatermark.
    unfold e1_default_flip_safe in Hsafe.
    destruct Hsafe as
      [Hdefault [_ [Htrigger [Hposts [Hfailed [Hcloses Hreleases]]]]]].
    eapply
      (E1Default.default_flip_fanout_trigger_progress
        (e1_index_mode c = true)
        (e1_dedicated c = true)
        (e1_fanout_watermark c = true)
        (e1_trigger_sent c = true)
        (e1_trigger_failed c = true)
        (e1_driver_posted c = true)
        (e1_cycle_closed c = true)
        (e1_generation_advanced c = true)
        (e1_request_cleared c = true)
        (e1_workers_resumed c = true));
      eauto.
  Qed.

  Theorem legacy_default_ungated_exposes_e1_default_flip_gap :
    ~ e1_default_flip_safe legacy_default_ungated_e1_default_flip.
  Proof.
    intros [_ [Hlegacy _]].
    unfold e1_legacy_requests_suppressed,
      legacy_default_ungated_e1_default_flip in Hlegacy.
    simpl in Hlegacy.
    specialize (Hlegacy eq_refl E1DefaultSafepoint).
    apply Hlegacy.
    reflexivity.
  Qed.

  Theorem trigger_failure_missing_backstop_exposes_e1_default_flip_gap :
    ~ e1_default_flip_safe trigger_failure_missing_backstop_e1_default_flip.
  Proof.
    intros [_ [_ [_ [_ [Hfailed _]]]]].
    unfold e1_failed_trigger_backstopped,
      trigger_failure_missing_backstop_e1_default_flip in Hfailed.
    simpl in Hfailed.
    destruct (Hfailed eq_refl) as [Hcleared _].
    discriminate Hcleared.
  Qed.

  Record E1SatbStwDriverConfig : Type := {
    e1_satb_driver_posted : Prop;
    e1_satb_success : Prop;
    e1_satb_panic : Prop;
    e1_final_sweep_closed : Prop;
    e1_satb_abort : Prop;
    e1_stw_requested : Prop;
    e1_stw_ran : Prop;
    e1_initial_cycle_closed : Prop;
    e1_final_cycle_closed : Prop;
    e1_stw_cycle_closed : Prop;
    e1_driver_generation_advanced : Prop;
    e1_driver_request_cleared : Prop;
    e1_driver_workers_resumed : Prop;
    e1_driver_witness_cleared : Prop;
    e1_driver_gc_in_progress_released : Prop
  }.

  Definition e1_satb_cycle_release
      (c : E1SatbStwDriverConfig)
      (closed : Prop)
      : Prop :=
    closed ->
    e1_driver_generation_advanced c /\
    e1_driver_request_cleared c /\
    e1_driver_workers_resumed c /\
    e1_driver_witness_cleared c /\
    e1_driver_gc_in_progress_released c.

  Definition e1_satb_driver_released
      (c : E1SatbStwDriverConfig)
      : Prop :=
    e1_driver_generation_advanced c /\
    e1_driver_request_cleared c /\
    e1_driver_workers_resumed c /\
    e1_driver_witness_cleared c /\
    e1_driver_gc_in_progress_released c.

  Definition e1_satb_abort_detected
      (c : E1SatbStwDriverConfig)
      : Prop :=
    e1_satb_panic c \/ e1_final_sweep_closed c.

  Definition e1_satb_driver_case_total
      (c : E1SatbStwDriverConfig)
      : Prop :=
    e1_satb_driver_posted c ->
    e1_satb_success c \/ e1_satb_abort c.

  Definition e1_satb_success_closes_cycles
      (c : E1SatbStwDriverConfig)
      : Prop :=
    e1_satb_success c ->
    e1_initial_cycle_closed c /\ e1_final_cycle_closed c.

  Definition e1_satb_panic_or_closed_final_sweep_aborts
      (c : E1SatbStwDriverConfig)
      : Prop :=
    (e1_satb_panic c -> e1_satb_abort c) /\
    (e1_final_sweep_closed c -> e1_satb_abort c).

  Definition e1_satb_abort_posts_fresh_stw
      (c : E1SatbStwDriverConfig)
      : Prop :=
    e1_satb_abort c -> e1_stw_requested c.

  Definition e1_stw_backstop_closes
      (c : E1SatbStwDriverConfig)
      : Prop :=
    e1_stw_requested c -> e1_stw_ran c /\ e1_stw_cycle_closed c.

  Definition e1_satb_stw_driver_safe
      (c : E1SatbStwDriverConfig)
      : Prop :=
    e1_satb_driver_case_total c /\
    e1_satb_success_closes_cycles c /\
    e1_satb_panic_or_closed_final_sweep_aborts c /\
    e1_satb_cycle_release c (e1_final_cycle_closed c) /\
    e1_satb_abort_posts_fresh_stw c /\
    e1_stw_backstop_closes c /\
    e1_satb_cycle_release c (e1_stw_cycle_closed c).

  Definition complete_e1_satb_stw_driver : E1SatbStwDriverConfig :=
    {| e1_satb_driver_posted := True;
       e1_satb_success := True;
       e1_satb_panic := False;
       e1_final_sweep_closed := False;
       e1_satb_abort := False;
       e1_stw_requested := False;
       e1_stw_ran := False;
       e1_initial_cycle_closed := True;
       e1_final_cycle_closed := True;
       e1_stw_cycle_closed := False;
       e1_driver_generation_advanced := True;
       e1_driver_request_cleared := True;
       e1_driver_workers_resumed := True;
       e1_driver_witness_cleared := True;
       e1_driver_gc_in_progress_released := True |}.

  Definition missing_e1_satb_success_release_driver
      : E1SatbStwDriverConfig :=
    {| e1_satb_driver_posted := True;
       e1_satb_success := True;
       e1_satb_panic := False;
       e1_final_sweep_closed := False;
       e1_satb_abort := False;
       e1_stw_requested := False;
       e1_stw_ran := False;
       e1_initial_cycle_closed := True;
       e1_final_cycle_closed := True;
       e1_stw_cycle_closed := False;
       e1_driver_generation_advanced := True;
       e1_driver_request_cleared := True;
       e1_driver_workers_resumed := True;
       e1_driver_witness_cleared := False;
       e1_driver_gc_in_progress_released := True |}.

  Definition missing_e1_satb_abort_stw_backstop_driver
      : E1SatbStwDriverConfig :=
    {| e1_satb_driver_posted := True;
       e1_satb_success := False;
       e1_satb_panic := True;
       e1_final_sweep_closed := False;
       e1_satb_abort := True;
       e1_stw_requested := False;
       e1_stw_ran := False;
       e1_initial_cycle_closed := False;
       e1_final_cycle_closed := False;
       e1_stw_cycle_closed := False;
       e1_driver_generation_advanced := True;
       e1_driver_request_cleared := True;
       e1_driver_workers_resumed := True;
       e1_driver_witness_cleared := True;
       e1_driver_gc_in_progress_released := True |}.

  Theorem complete_e1_satb_stw_driver_safe :
    e1_satb_stw_driver_safe complete_e1_satb_stw_driver.
  Proof.
    unfold e1_satb_stw_driver_safe, e1_satb_driver_case_total,
      e1_satb_success_closes_cycles,
      e1_satb_panic_or_closed_final_sweep_aborts,
      e1_satb_cycle_release, e1_satb_abort_posts_fresh_stw,
      e1_stw_backstop_closes, complete_e1_satb_stw_driver.
    simpl.
    repeat split; intros; try contradiction; try (left; exact I);
      repeat split; exact I.
  Qed.

  Theorem e1_satb_stw_driver_safe_implies_released :
    forall c,
      e1_satb_stw_driver_safe c ->
      e1_satb_driver_posted c ->
      e1_satb_driver_released c.
  Proof.
    intros c Hsafe Hposted.
    unfold e1_satb_stw_driver_safe in Hsafe.
    destruct Hsafe as
      [Hcase [Hsuccess [_ [Hfinal_release
        [Habort_posts [Hbackstop Hstw_release]]]]]].
    unfold e1_satb_driver_released.
    eapply E1Driver.posted_driver_satb_or_stw_releases.
    - exact Hcase.
    - exact Hsuccess.
    - exact Hfinal_release.
    - exact Habort_posts.
    - exact Hbackstop.
    - exact Hstw_release.
    - exact Hposted.
  Qed.

  Theorem e1_satb_stw_driver_safe_clears_sticky_request_and_witness :
    forall c,
      e1_satb_stw_driver_safe c ->
      e1_satb_driver_posted c ->
      e1_driver_request_cleared c /\
      e1_driver_workers_resumed c /\
      e1_driver_witness_cleared c /\
      e1_driver_gc_in_progress_released c.
  Proof.
    intros c Hsafe Hposted.
    unfold e1_satb_stw_driver_safe in Hsafe.
    destruct Hsafe as
      [Hcase [Hsuccess [_ [Hfinal_release
        [Habort_posts [Hbackstop Hstw_release]]]]]].
    eapply E1Driver.posted_driver_no_sticky_request_or_witness.
    - exact Hcase.
    - exact Hsuccess.
    - exact Hfinal_release.
    - exact Habort_posts.
    - exact Hbackstop.
    - exact Hstw_release.
    - exact Hposted.
  Qed.

  Theorem panic_or_closed_final_sweep_exposes_driver_abort :
    forall c,
      e1_satb_stw_driver_safe c ->
      e1_satb_abort_detected c ->
      e1_satb_abort c.
  Proof.
    intros c Hsafe Habort_detected.
    unfold e1_satb_stw_driver_safe in Hsafe.
    destruct Hsafe as [_ [_ [Haborts _]]].
    destruct Haborts as [Hpanic Hclosed].
    eapply E1Driver.panic_or_closed_final_sweep_is_satb_abort.
    - exact Hpanic.
    - exact Hclosed.
    - exact Habort_detected.
  Qed.

  Theorem missing_e1_satb_success_release_exposes_driver_gap :
    ~ e1_satb_stw_driver_safe missing_e1_satb_success_release_driver.
  Proof.
    intros [_ [_ [_ [Hfinal_release _]]]].
    unfold e1_satb_cycle_release,
      missing_e1_satb_success_release_driver in Hfinal_release.
    simpl in Hfinal_release.
    destruct (Hfinal_release I) as [_ [_ [_ [Hwitness _]]]].
    exact Hwitness.
  Qed.

  Theorem missing_e1_satb_abort_stw_backstop_exposes_driver_gap :
    ~ e1_satb_stw_driver_safe missing_e1_satb_abort_stw_backstop_driver.
  Proof.
    intros [_ [_ [_ [_ [Habort_posts _]]]]].
    unfold e1_satb_abort_posts_fresh_stw,
      missing_e1_satb_abort_stw_backstop_driver in Habort_posts.
    simpl in Habort_posts.
    exact (Habort_posts I).
  Qed.

  Record DedicatedHandoffConfig : Type := {
    dedicated_sent : Prop;
    dedicated_roots_available : Prop;
    dedicated_response_failed : Prop;
    dedicated_returned_err : Prop;
    dedicated_returned_ok_false : Prop;
    dedicated_inline_fallback : Prop;
    dedicated_response_sender_carried : Prop;
    dedicated_handler_runs : Prop;
    dedicated_reply_attempted : Prop;
    dedicated_collection_panicked : Prop;
    dedicated_collection_returned : Prop
  }.

  Definition dedicated_inline_fallback_safe
      (c : DedicatedHandoffConfig)
      : Prop :=
    Dedicated.InlineFallbackSafe
      (dedicated_inline_fallback c)
      (dedicated_roots_available c).

  Definition dedicated_response_failure_skip_safe
      (c : DedicatedHandoffConfig)
      : Prop :=
    dedicated_response_failed c -> dedicated_returned_ok_false c.

  Definition dedicated_skip_is_not_error
      (c : DedicatedHandoffConfig)
      : Prop :=
    dedicated_returned_ok_false c -> ~ dedicated_returned_err c.

  Definition dedicated_inline_fallback_is_error
      (c : DedicatedHandoffConfig)
      : Prop :=
    dedicated_inline_fallback c -> dedicated_returned_err c.

  Definition dedicated_sent_consumes_roots
      (c : DedicatedHandoffConfig)
      : Prop :=
    dedicated_sent c -> ~ dedicated_roots_available c.

  Definition dedicated_collect_reply_producer_safe
      (c : DedicatedHandoffConfig)
      : Prop :=
    Dedicated.CollectReplyProducerSafe
      (dedicated_sent c)
      (dedicated_response_sender_carried c)
      (dedicated_handler_runs c)
      (dedicated_reply_attempted c).

  Definition dedicated_handler_catches_collection_result
      (c : DedicatedHandoffConfig)
      : Prop :=
    dedicated_handler_runs c ->
    dedicated_collection_panicked c \/ dedicated_collection_returned c.

  Definition dedicated_handoff_safe
      (c : DedicatedHandoffConfig)
      : Prop :=
    dedicated_sent_consumes_roots c /\
    dedicated_inline_fallback_safe c /\
    dedicated_response_failure_skip_safe c /\
    dedicated_skip_is_not_error c /\
    dedicated_inline_fallback_is_error c /\
    dedicated_collect_reply_producer_safe c /\
    dedicated_handler_catches_collection_result c.

  Definition complete_dedicated_handoff : DedicatedHandoffConfig :=
    {| dedicated_sent := True;
       dedicated_roots_available := False;
       dedicated_response_failed := False;
       dedicated_returned_err := False;
       dedicated_returned_ok_false := False;
       dedicated_inline_fallback := False;
       dedicated_response_sender_carried := True;
       dedicated_handler_runs := True;
       dedicated_reply_attempted := True;
       dedicated_collection_panicked := False;
       dedicated_collection_returned := True |}.

  Definition inline_after_consumed_dedicated_handoff
      : DedicatedHandoffConfig :=
    {| dedicated_sent := True;
       dedicated_roots_available := False;
       dedicated_response_failed := True;
       dedicated_returned_err := True;
       dedicated_returned_ok_false := False;
       dedicated_inline_fallback := True;
       dedicated_response_sender_carried := True;
       dedicated_handler_runs := True;
       dedicated_reply_attempted := True;
       dedicated_collection_panicked := False;
       dedicated_collection_returned := True |}.

  Definition missing_reply_dedicated_handoff : DedicatedHandoffConfig :=
    {| dedicated_sent := True;
       dedicated_roots_available := False;
       dedicated_response_failed := False;
       dedicated_returned_err := False;
       dedicated_returned_ok_false := False;
       dedicated_inline_fallback := False;
       dedicated_response_sender_carried := True;
       dedicated_handler_runs := True;
       dedicated_reply_attempted := False;
       dedicated_collection_panicked := False;
       dedicated_collection_returned := True |}.

  Theorem complete_dedicated_handoff_safe :
    dedicated_handoff_safe complete_dedicated_handoff.
  Proof.
    unfold dedicated_handoff_safe, dedicated_sent_consumes_roots,
      dedicated_inline_fallback_safe, dedicated_response_failure_skip_safe,
      dedicated_skip_is_not_error, dedicated_inline_fallback_is_error,
      dedicated_collect_reply_producer_safe,
      dedicated_handler_catches_collection_result,
      complete_dedicated_handoff.
    simpl.
    repeat split.
    - intros _ Hroots.
      exact Hroots.
    - intros Hinline.
      exact Hinline.
    - intros Hfailed.
      exact Hfailed.
    - intros _ Herr.
      exact Herr.
    - intros Hinline.
      exact Hinline.
    - intros _.
      right.
      exact I.
  Qed.

  Theorem dedicated_handoff_safe_forbids_inline_after_consumed_roots :
    forall c,
      dedicated_handoff_safe c ->
      dedicated_sent c ->
      ~ dedicated_inline_fallback c.
  Proof.
    intros c Hsafe Hsent.
    unfold dedicated_handoff_safe in Hsafe.
    destruct Hsafe as [Hsent_consumes [Hinline_safe _]].
    eapply Dedicated.consumed_roots_forbid_inline_fallback.
    - exact Hsent_consumes.
    - exact Hinline_safe.
    - exact Hsent.
  Qed.

  Theorem dedicated_handoff_response_failure_after_send_skip_only :
    forall c,
      dedicated_handoff_safe c ->
      dedicated_sent c ->
      dedicated_response_failed c ->
      ~ dedicated_inline_fallback c.
  Proof.
    intros c Hsafe Hsent Hfailed.
    unfold dedicated_handoff_safe in Hsafe.
    destruct Hsafe as
      [Hsent_consumes [_ [Hfailed_ok [Hok_not_err
        [Hinline_err _]]]]].
    eapply Dedicated.response_failure_after_send_is_skip_only.
    - exact Hsent_consumes.
    - exact Hfailed_ok.
    - exact Hok_not_err.
    - exact Hinline_err.
    - exact Hsent.
    - exact Hfailed.
  Qed.

  Theorem dedicated_handoff_safe_has_reply_producer :
    forall c,
      dedicated_handoff_safe c ->
      dedicated_sent c ->
      dedicated_response_sender_carried c /\
      dedicated_handler_runs c /\
      dedicated_reply_attempted c.
  Proof.
    intros c Hsafe Hsent.
    unfold dedicated_handoff_safe in Hsafe.
    destruct Hsafe as [_ [_ [_ [_ [_ [Hreply _]]]]]].
    apply Hreply.
    exact Hsent.
  Qed.

  Theorem dedicated_handoff_caught_result_still_replies :
    forall c,
      dedicated_handoff_safe c ->
      dedicated_sent c ->
      dedicated_reply_attempted c.
  Proof.
    intros c Hsafe Hsent.
    unfold dedicated_handoff_safe,
      dedicated_collect_reply_producer_safe in Hsafe.
    destruct Hsafe as [_ [_ [_ [_ [_ [Hreply Hcaught]]]]]].
    eapply Dedicated.caught_collection_result_still_replies.
    - intros _.
      destruct (Hreply Hsent) as [_ [Hhandler _]].
      exact Hhandler.
    - exact Hcaught.
    - intros _.
      destruct (Hreply Hsent) as [_ [_ Hattempted]].
      exact Hattempted.
    - exact Hsent.
  Qed.

  Theorem inline_after_consumed_exposes_dedicated_handoff_gap :
    ~ dedicated_handoff_safe inline_after_consumed_dedicated_handoff.
  Proof.
    intros [_ [Hinline_safe _]].
    unfold dedicated_inline_fallback_safe,
      inline_after_consumed_dedicated_handoff in Hinline_safe.
    simpl in Hinline_safe.
    exact (Hinline_safe I).
  Qed.

  Theorem missing_reply_exposes_dedicated_handoff_gap :
    ~ dedicated_handoff_safe missing_reply_dedicated_handoff.
  Proof.
    intros [_ [_ [_ [_ [_ [Hreply _]]]]]].
    unfold dedicated_collect_reply_producer_safe,
      missing_reply_dedicated_handoff in Hreply.
    simpl in Hreply.
    destruct (Hreply I) as [_ [_ Hattempted]].
    exact Hattempted.
  Qed.

  Record DriverChannelConfig : Type := {
    channel_request_sender_stored : Prop;
    channel_request_receiver_owned : Prop;
    channel_collect_sent : Prop;
    channel_response_sender_carried : Prop;
    channel_response_receiver_owned : Prop;
    channel_driver_received_collect : Prop;
    channel_reply_attempted : Prop;
    channel_caller_waiting : Prop;
    channel_fire_and_forget_sent : Prop
  }.

  Definition channel_received_has_receiver
      (c : DriverChannelConfig)
      : Prop :=
    channel_driver_received_collect c ->
    channel_request_receiver_owned c.

  Definition channel_receiver_has_sender
      (c : DriverChannelConfig)
      : Prop :=
    channel_request_receiver_owned c ->
    channel_request_sender_stored c.

  Definition channel_wait_sent_collect
      (c : DriverChannelConfig)
      : Prop :=
    channel_caller_waiting c -> channel_collect_sent c.

  Definition channel_collect_carries_response_sender
      (c : DriverChannelConfig)
      : Prop :=
    channel_collect_sent c -> channel_response_sender_carried c.

  Definition channel_collect_has_response_receiver
      (c : DriverChannelConfig)
      : Prop :=
    channel_collect_sent c -> channel_response_receiver_owned c.

  Definition channel_collect_reaches_driver
      (c : DriverChannelConfig)
      : Prop :=
    channel_collect_sent c -> channel_driver_received_collect c.

  Definition channel_driver_reply_attempted
      (c : DriverChannelConfig)
      : Prop :=
    channel_driver_received_collect c -> channel_reply_attempted c.

  Definition channel_reply_from_driver_receive
      (c : DriverChannelConfig)
      : Prop :=
    channel_reply_attempted c -> channel_driver_received_collect c.

  Definition channel_driver_receive_from_collect
      (c : DriverChannelConfig)
      : Prop :=
    channel_driver_received_collect c -> channel_collect_sent c.

  Definition channel_reply_has_waiter
      (c : DriverChannelConfig)
      : Prop :=
    channel_reply_attempted c -> channel_caller_waiting c.

  Definition channel_fire_and_forget_no_wait
      (c : DriverChannelConfig)
      : Prop :=
    channel_fire_and_forget_sent c -> ~ channel_caller_waiting c.

  Definition channel_fire_and_forget_no_reply
      (c : DriverChannelConfig)
      : Prop :=
    channel_fire_and_forget_sent c -> ~ channel_reply_attempted c.

  Definition driver_channel_protocol_safe
      (c : DriverChannelConfig)
      : Prop :=
    channel_received_has_receiver c /\
    channel_receiver_has_sender c /\
    channel_wait_sent_collect c /\
    channel_collect_carries_response_sender c /\
    channel_collect_has_response_receiver c /\
    channel_collect_reaches_driver c /\
    channel_driver_reply_attempted c /\
    channel_reply_from_driver_receive c /\
    channel_driver_receive_from_collect c /\
    channel_reply_has_waiter c /\
    channel_fire_and_forget_no_wait c /\
    channel_fire_and_forget_no_reply c.

  Definition complete_driver_channel : DriverChannelConfig :=
    {| channel_request_sender_stored := True;
       channel_request_receiver_owned := True;
       channel_collect_sent := True;
       channel_response_sender_carried := True;
       channel_response_receiver_owned := True;
       channel_driver_received_collect := True;
       channel_reply_attempted := True;
       channel_caller_waiting := True;
       channel_fire_and_forget_sent := False |}.

  Definition missing_request_sender_driver_channel : DriverChannelConfig :=
    {| channel_request_sender_stored := False;
       channel_request_receiver_owned := True;
       channel_collect_sent := True;
       channel_response_sender_carried := True;
       channel_response_receiver_owned := True;
       channel_driver_received_collect := True;
       channel_reply_attempted := True;
       channel_caller_waiting := True;
       channel_fire_and_forget_sent := False |}.

  Definition missing_reply_driver_channel : DriverChannelConfig :=
    {| channel_request_sender_stored := True;
       channel_request_receiver_owned := True;
       channel_collect_sent := True;
       channel_response_sender_carried := True;
       channel_response_receiver_owned := True;
       channel_driver_received_collect := True;
       channel_reply_attempted := False;
       channel_caller_waiting := True;
       channel_fire_and_forget_sent := False |}.

  Definition orphan_reply_driver_channel : DriverChannelConfig :=
    {| channel_request_sender_stored := True;
       channel_request_receiver_owned := True;
       channel_collect_sent := True;
       channel_response_sender_carried := True;
       channel_response_receiver_owned := True;
       channel_driver_received_collect := True;
       channel_reply_attempted := True;
       channel_caller_waiting := False;
       channel_fire_and_forget_sent := False |}.

  Definition fire_and_forget_waits_driver_channel : DriverChannelConfig :=
    {| channel_request_sender_stored := True;
       channel_request_receiver_owned := True;
       channel_collect_sent := True;
       channel_response_sender_carried := True;
       channel_response_receiver_owned := True;
       channel_driver_received_collect := True;
       channel_reply_attempted := True;
       channel_caller_waiting := True;
       channel_fire_and_forget_sent := True |}.

  Theorem complete_driver_channel_protocol_safe :
    driver_channel_protocol_safe complete_driver_channel.
  Proof.
    unfold driver_channel_protocol_safe, channel_received_has_receiver,
      channel_receiver_has_sender, channel_wait_sent_collect,
      channel_collect_carries_response_sender,
      channel_collect_has_response_receiver, channel_collect_reaches_driver,
      channel_driver_reply_attempted, channel_reply_from_driver_receive,
      channel_driver_receive_from_collect, channel_reply_has_waiter,
      channel_fire_and_forget_no_wait, channel_fire_and_forget_no_reply,
      complete_driver_channel.
    simpl.
    repeat split; intros; try contradiction; exact I.
  Qed.

  Theorem driver_channel_protocol_safe_has_request_receive_producer :
    forall c,
      driver_channel_protocol_safe c ->
      DriverChannel.RequestReceiveSafe
        (channel_driver_received_collect c)
        (channel_request_sender_stored c)
        (channel_request_receiver_owned c).
  Proof.
    intros c Hsafe.
    unfold driver_channel_protocol_safe in Hsafe.
    destruct Hsafe as [Hreceived [Hreceiver _]].
    eapply DriverChannel.spawned_request_receive_has_producer.
    - exact Hreceived.
    - exact Hreceiver.
  Qed.

  Theorem driver_channel_protocol_safe_has_response_wait_producer :
    forall c,
      driver_channel_protocol_safe c ->
      DriverChannel.ResponseWaitSafe
        (channel_caller_waiting c)
        (channel_response_sender_carried c)
        (channel_response_receiver_owned c)
        (channel_reply_attempted c).
  Proof.
    intros c Hsafe.
    unfold driver_channel_protocol_safe in Hsafe.
    destruct Hsafe as
      [_ [_ [Hwait [Hsender [Hreceiver [Hreaches [Hreply _]]]]]]].
    eapply DriverChannel.successful_collect_wait_has_response_producer.
    - exact Hwait.
    - exact Hsender.
    - exact Hreceiver.
    - exact Hreaches.
    - exact Hreply.
  Qed.

  Theorem driver_channel_protocol_safe_has_no_orphan_reply :
    forall c,
      driver_channel_protocol_safe c ->
      DriverChannel.ReplySendNotOrphaned
        (channel_reply_attempted c)
        (channel_response_sender_carried c)
        (channel_response_receiver_owned c)
        (channel_caller_waiting c).
  Proof.
    intros c Hsafe.
    unfold driver_channel_protocol_safe in Hsafe.
    destruct Hsafe as
      [_ [_ [_ [Hsender [Hreceiver [_ [_ [Hreply_driver
        [Hdriver_collect [Hreply_wait _]]]]]]]]]].
    eapply DriverChannel.reply_attempt_is_not_orphaned.
    - exact Hreply_driver.
    - exact Hdriver_collect.
    - exact Hsender.
    - exact Hreceiver.
    - exact Hreply_wait.
  Qed.

  Theorem driver_channel_protocol_safe_fire_and_forget_no_wait :
    forall c,
      driver_channel_protocol_safe c ->
      DriverChannel.FireAndForgetSafe
        (channel_fire_and_forget_sent c)
        (channel_caller_waiting c)
        (channel_reply_attempted c).
  Proof.
    intros c Hsafe.
    unfold driver_channel_protocol_safe in Hsafe.
    destruct Hsafe as
      [_ [_ [_ [_ [_ [_ [_ [_ [_ [_ [Hno_wait Hno_reply]]]]]]]]]]].
    eapply DriverChannel.fire_and_forget_request_has_no_response_wait.
    - exact Hno_wait.
    - exact Hno_reply.
  Qed.

  Theorem driver_channel_protocol_safe_exports_collect_wait :
    forall c,
      driver_channel_protocol_safe c ->
      channel_caller_waiting c ->
      (channel_request_sender_stored c /\
       channel_request_receiver_owned c) /\
      (channel_response_sender_carried c /\
       channel_response_receiver_owned c /\
       channel_reply_attempted c).
  Proof.
    intros c Hsafe Hwaiting.
    unfold driver_channel_protocol_safe in Hsafe.
    destruct Hsafe as
      [Hreceived [Hreceiver [Hwait [Hsender [Hresponse_receiver
        [Hreaches [Hdriver_reply _]]]]]]].
    eapply DriverChannel.gc_driver_channel_protocol_safe.
    - exact Hreceived.
    - exact Hreceiver.
    - exact Hwait.
    - exact Hsender.
    - exact Hresponse_receiver.
    - exact Hreaches.
    - exact Hdriver_reply.
    - exact Hwaiting.
  Qed.

  Theorem missing_request_sender_exposes_driver_channel_gap :
    ~ driver_channel_protocol_safe missing_request_sender_driver_channel.
  Proof.
    intros [_ [Hreceiver _]].
    unfold channel_receiver_has_sender,
      missing_request_sender_driver_channel in Hreceiver.
    simpl in Hreceiver.
    exact (Hreceiver I).
  Qed.

  Theorem missing_reply_exposes_driver_channel_gap :
    ~ driver_channel_protocol_safe missing_reply_driver_channel.
  Proof.
    intros [_ [_ [_ [_ [_ [_ [Hreply _]]]]]]].
    unfold channel_driver_reply_attempted,
      missing_reply_driver_channel in Hreply.
    simpl in Hreply.
    exact (Hreply I).
  Qed.

  Theorem orphan_reply_exposes_driver_channel_gap :
    ~ driver_channel_protocol_safe orphan_reply_driver_channel.
  Proof.
    intros [_ [_ [_ [_ [_ [_ [_ [_ [_ [Hwait _]]]]]]]]]].
    unfold channel_reply_has_waiter, orphan_reply_driver_channel in Hwait.
    simpl in Hwait.
    exact (Hwait I).
  Qed.

  Theorem fire_and_forget_wait_exposes_driver_channel_gap :
    ~ driver_channel_protocol_safe fire_and_forget_waits_driver_channel.
  Proof.
    intros [_ [_ [_ [_ [_ [_ [_ [_ [_ [_ [Hno_wait _]]]]]]]]]]].
    unfold channel_fire_and_forget_no_wait,
      fire_and_forget_waits_driver_channel in Hno_wait.
    simpl in Hno_wait.
    exact (Hno_wait I I).
  Qed.

  Definition WaveAssignment : Type := Task -> nat.

  Definition dependency_order_sound
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    has_dependency w = true ->
    dependency_edge_encoded w = true /\
    wave Producer < wave Consumer.

  Definition effect_conflict_sound
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    has_effect_conflict w = true ->
    effect_conflict_edge_encoded w = true /\
    wave Producer <> wave Consumer.

  Definition independent_parallelism_maximal
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    has_dependency w = false ->
    has_effect_conflict w = false ->
    wave Producer = wave Consumer.

  Definition direct_fanout_sound
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    uses_direct_fanout w = true ->
    has_dependency w = false /\
    has_effect_conflict w = false /\
    wave Producer = wave Consumer.

  Definition workload_dependency_edge
      (w : Workload)
      (dep task : Task)
      : Prop :=
    has_dependency w = true /\
    dependency_edge_encoded w = true /\
    dep = Producer /\
    task = Consumer.

  Definition workload_effect_conflict
      (w : Workload)
      (left right : Task)
      : Prop :=
    has_effect_conflict w = true /\
    ((left = Producer /\ right = Consumer) \/
     (left = Consumer /\ right = Producer)).

  Definition workload_effect_order_edge
      (w : Workload)
      (wave : WaveAssignment)
      (dep task : Task)
      : Prop :=
    has_effect_conflict w = true /\
    effect_conflict_edge_encoded w = true /\
    ((dep = Producer /\ task = Consumer /\ wave Producer < wave Consumer) \/
     (dep = Consumer /\ task = Producer /\ wave Consumer < wave Producer)).

  Definition wavefront_dependency_order_safe
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    @Wavefront.dependencies_before Task wave (workload_dependency_edge w).

  Definition wavefront_same_wave_dependency_independent
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    @Wavefront.same_wave_independent Task wave (workload_dependency_edge w).

  Definition effect_conflict_order_safe
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    @EffectCompleteness.dependencies_before
      Task wave (workload_effect_order_edge w wave).

  Definition effect_conflict_edges_covered
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    @EffectCompleteness.conflict_edges_covered
      Task (workload_effect_order_edge w wave) (workload_effect_conflict w).

  Definition same_wave_effect_conflict_free
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    @EffectCompleteness.same_wave_conflict_free
      Task wave (workload_effect_conflict w).

  Definition direct_dependency_edges (w : Workload) : nat :=
    if has_dependency w then 1 else 0.

  Definition direct_fanout_wavefront_refinement_safe
      (w : Workload)
      : Prop :=
    uses_direct_fanout w = true ->
    DirectRefinement.direct_fanout_refines_independent_wavefront
      2
      (direct_dependency_edges w)
      1
      2
      2.

  Definition standalone_scheduler_reordering_safe
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    wavefront_dependency_order_safe w wave /\
    wavefront_same_wave_dependency_independent w wave /\
    effect_conflict_order_safe w wave /\
    effect_conflict_edges_covered w wave /\
    same_wave_effect_conflict_free w wave /\
    direct_fanout_wavefront_refinement_safe w.

  Definition schedule_envelope_safe
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    dependency_order_sound w wave /\
    effect_conflict_sound w wave /\
    independent_parallelism_maximal w wave /\
    direct_fanout_sound w wave /\
    standalone_scheduler_reordering_safe w wave.

  Theorem dependency_order_sound_implies_wavefront_order :
    forall w wave,
      dependency_order_sound w wave ->
      wavefront_dependency_order_safe w wave.
  Proof.
    intros w wave Hsound.
    unfold wavefront_dependency_order_safe, workload_dependency_edge.
    unfold Wavefront.dependencies_before.
    intros task dep [Hhas_dependency [_ [Hdep Htask]]].
    destruct task; destruct dep; try discriminate.
    subst.
    specialize (Hsound Hhas_dependency) as [_ Hbefore].
    exact Hbefore.
  Qed.

  Theorem dependency_order_sound_implies_same_wave_independent :
    forall w wave,
      dependency_order_sound w wave ->
      wavefront_same_wave_dependency_independent w wave.
  Proof.
    intros w wave Hsound.
    unfold wavefront_same_wave_dependency_independent.
    apply Wavefront.dependencies_before_implies_same_wave_independent.
    apply dependency_order_sound_implies_wavefront_order.
    exact Hsound.
  Qed.

  Theorem effect_conflict_sound_implies_order_safe :
    forall w wave,
      effect_conflict_sound w wave ->
      effect_conflict_order_safe w wave.
  Proof.
    intros w wave Hsound.
    unfold effect_conflict_order_safe, workload_effect_order_edge.
    unfold EffectCompleteness.dependencies_before.
    intros task dep [_ [_ [[Hdep [Htask Hbefore]] |
                          [Hdep [Htask Hbefore]]]]].
    - subst.
      exact Hbefore.
    - subst.
      exact Hbefore.
  Qed.

  Theorem effect_conflict_sound_implies_edges_covered :
    forall w wave,
      effect_conflict_sound w wave ->
      effect_conflict_edges_covered w wave.
  Proof.
    intros w wave Hsound.
    unfold effect_conflict_edges_covered, workload_effect_conflict,
      workload_effect_order_edge.
    unfold EffectCompleteness.conflict_edges_covered.
    intros left right [Hconflict [[Hleft Hright] | [Hleft Hright]]].
    - subst.
      specialize (Hsound Hconflict) as [Hencoded Hdistinct].
      destruct (Nat.lt_ge_cases (wave Producer) (wave Consumer)) as
        [Hbefore | Hnot_before].
      + left.
        split.
        * exact Hconflict.
        * split.
          -- exact Hencoded.
          -- left.
             repeat split; try reflexivity; exact Hbefore.
      + right.
        split.
        * exact Hconflict.
        * split.
          -- exact Hencoded.
          -- right.
             repeat split; try reflexivity; lia.
    - subst.
      specialize (Hsound Hconflict) as [Hencoded Hdistinct].
      destruct (Nat.lt_ge_cases (wave Producer) (wave Consumer)) as
        [Hbefore | Hnot_before].
      + right.
        split.
        * exact Hconflict.
        * split.
          -- exact Hencoded.
          -- left.
             repeat split; try reflexivity; exact Hbefore.
      + left.
        split.
        * exact Hconflict.
        * split.
          -- exact Hencoded.
          -- right.
             repeat split; try reflexivity; lia.
  Qed.

  Theorem effect_conflict_sound_implies_same_wave_conflict_free :
    forall w wave,
      effect_conflict_sound w wave ->
      same_wave_effect_conflict_free w wave.
  Proof.
    intros w wave Hsound.
    unfold same_wave_effect_conflict_free.
    apply EffectCompleteness.dependency_order_and_conflict_coverage_imply_same_wave_conflict_free
      with (depends_on := workload_effect_order_edge w wave).
    - apply effect_conflict_sound_implies_order_safe.
      exact Hsound.
    - apply effect_conflict_sound_implies_edges_covered.
      exact Hsound.
  Qed.

  Theorem direct_fanout_sound_implies_wavefront_refinement :
    forall w wave,
      direct_fanout_sound w wave ->
      direct_fanout_wavefront_refinement_safe w.
  Proof.
    intros w wave Hsound Hdirect.
    specialize (Hsound Hdirect) as [Hnodep _].
    unfold direct_fanout_wavefront_refinement_safe, direct_dependency_edges.
    rewrite Hnodep.
    apply DirectRefinement.independent_complete_direct_fanout_refines_single_wave.
    - lia.
    - unfold DirectRefinement.direct_fanout_complete.
      reflexivity.
  Qed.

  Theorem local_scheduler_contracts_compose_standalone_reordering :
    forall w wave,
      dependency_order_sound w wave ->
      effect_conflict_sound w wave ->
      direct_fanout_sound w wave ->
      standalone_scheduler_reordering_safe w wave.
  Proof.
    intros w wave Hdep Hconflict Hdirect.
    unfold standalone_scheduler_reordering_safe.
    split.
    - apply dependency_order_sound_implies_wavefront_order.
      exact Hdep.
    - split.
      + apply dependency_order_sound_implies_same_wave_independent.
        exact Hdep.
      + split.
        * apply effect_conflict_sound_implies_order_safe.
          exact Hconflict.
        * split.
          -- apply effect_conflict_sound_implies_edges_covered.
             exact Hconflict.
          -- split.
             ++ apply effect_conflict_sound_implies_same_wave_conflict_free.
                exact Hconflict.
             ++ apply direct_fanout_sound_implies_wavefront_refinement
                  with (wave := wave).
                exact Hdirect.
  Qed.

  Theorem complete_wave_contract_is_schedule_safe :
    forall w wave,
      dependency_order_sound w wave ->
      effect_conflict_sound w wave ->
      independent_parallelism_maximal w wave ->
      direct_fanout_sound w wave ->
      schedule_envelope_safe w wave.
  Proof.
    intros w wave Hdep Hconflict Hmax Hdirect.
    unfold schedule_envelope_safe.
    split.
    - exact Hdep.
    - split.
      + exact Hconflict.
      + split.
        * exact Hmax.
        * split.
          -- exact Hdirect.
          -- apply local_scheduler_contracts_compose_standalone_reordering;
             assumption.
  Qed.

  Theorem same_wave_dependency_exposes_reorder :
    forall w wave,
      has_dependency w = true ->
      wave Producer = wave Consumer ->
      ~ dependency_order_sound w wave.
  Proof.
    intros w wave Hdep Hsame Hsound.
    unfold dependency_order_sound in Hsound.
    specialize (Hsound Hdep) as [_ Hsound].
    lia.
  Qed.

  Theorem same_wave_effect_conflict_exposes_unsound_parallelism :
    forall w wave,
      has_effect_conflict w = true ->
      wave Producer = wave Consumer ->
      ~ effect_conflict_sound w wave.
  Proof.
    intros w wave Hconflict Hsame Hsound.
    unfold effect_conflict_sound in Hsound.
    specialize (Hsound Hconflict) as [_ Hsound].
    exact (Hsound Hsame).
  Qed.

  Theorem missing_dependency_edge_exposes_incomplete_reordering :
    forall w wave,
      has_dependency w = true ->
      dependency_edge_encoded w = false ->
      ~ dependency_order_sound w wave.
  Proof.
    intros w wave Hdep Hmissing Hsound.
    unfold dependency_order_sound in Hsound.
    specialize (Hsound Hdep) as [Hencoded _].
    rewrite Hmissing in Hencoded.
    discriminate Hencoded.
  Qed.

  Theorem missing_effect_conflict_edge_exposes_incomplete_reordering :
    forall w wave,
      has_effect_conflict w = true ->
      effect_conflict_edge_encoded w = false ->
      ~ effect_conflict_sound w wave.
  Proof.
    intros w wave Hconflict Hmissing Hsound.
    unfold effect_conflict_sound in Hsound.
    specialize (Hsound Hconflict) as [Hencoded _].
    rewrite Hmissing in Hencoded.
    discriminate Hencoded.
  Qed.

  Theorem independent_workload_requires_same_wave_for_maximality :
    forall w wave,
      has_dependency w = false ->
      has_effect_conflict w = false ->
      independent_parallelism_maximal w wave ->
      wave Producer = wave Consumer.
  Proof.
    intros w wave Hdep Hconflict Hmax.
    unfold independent_parallelism_maximal in Hmax.
    exact (Hmax Hdep Hconflict).
  Qed.

  Theorem direct_fanout_rejects_dependency_bearing_workload :
    forall w wave,
      uses_direct_fanout w = true ->
      has_dependency w = true ->
      ~ direct_fanout_sound w wave.
  Proof.
    intros w wave Hdirect Hdep Hsound.
    unfold direct_fanout_sound in Hsound.
    destruct (Hsound Hdirect) as [Hnodep _].
    rewrite Hdep in Hnodep.
    discriminate.
  Qed.

  Definition sweep_frees_live_value
      (active_worker_live worker_rooted
       dispatch_live dispatch_rooted
       batch_live batch_rooted
       late_worker_live : bool)
      : bool :=
    (active_worker_live && negb worker_rooted) ||
    (dispatch_live && negb dispatch_rooted) ||
    (batch_live && negb batch_rooted) ||
    late_worker_live.

  Definition gc_window_safe
      (active_worker_live worker_rooted
       dispatch_live dispatch_rooted
       batch_live batch_rooted
       late_worker_live : bool)
      : Prop :=
    sweep_frees_live_value
      active_worker_live worker_rooted
      dispatch_live dispatch_rooted
      batch_live batch_rooted
      late_worker_live = false.

  Theorem rooted_closed_gc_window_safe :
    forall active_worker_live dispatch_live batch_live,
      gc_window_safe
        active_worker_live active_worker_live
        dispatch_live dispatch_live
        batch_live batch_live
        false.
  Proof.
    intros [] [] []; reflexivity.
  Qed.

  Theorem missing_active_worker_root_exposes_live_free :
    sweep_frees_live_value true false false false false false false = true.
  Proof.
    reflexivity.
  Qed.

  Theorem missing_dispatch_root_exposes_live_free :
    sweep_frees_live_value false false true false false false false = true.
  Proof.
    reflexivity.
  Qed.

  Theorem missing_batch_root_exposes_live_free :
    sweep_frees_live_value false false false false true false false = true.
  Proof.
    reflexivity.
  Qed.

  Theorem late_worker_after_snapshot_exposes_live_free :
    forall worker_rooted,
      sweep_frees_live_value
        false worker_rooted false false false false true = true.
  Proof.
    intros []; reflexivity.
  Qed.

  Definition boundary_root
      (live : bool)
      (target addr : BoundaryAddr)
      : Prop :=
    live = true /\ addr = target.

  Definition boundary_driver_root
      (worker_rooted dispatch_rooted batch_rooted : bool)
      (addr : BoundaryAddr)
      : Prop :=
    boundary_root worker_rooted BoundaryActiveWorker addr \/
    boundary_root dispatch_rooted BoundaryDispatchFanout addr \/
    boundary_root batch_rooted BoundaryBatchHandoff addr.

  Definition boundary_scheduler_live
      (active_worker_live dispatch_live batch_live late_worker_live : bool)
      : BoundaryAddr -> Prop :=
    @SchedulerLiveRoot BoundaryAddr
      (boundary_root active_worker_live BoundaryActiveWorker)
      (boundary_root dispatch_live BoundaryDispatchFanout)
      (boundary_root batch_live BoundaryBatchHandoff)
      (boundary_root late_worker_live BoundaryNewAdmission).

  Definition boundary_freed
      (active_worker_live worker_rooted
       dispatch_live dispatch_rooted
       batch_live batch_rooted
       late_worker_live : bool)
      (addr : BoundaryAddr)
      : Prop :=
    boundary_scheduler_live
      active_worker_live dispatch_live batch_live late_worker_live addr /\
    ~ boundary_driver_root worker_rooted dispatch_rooted batch_rooted addr.

  Definition scheduler_boundary_safe
      (active_worker_live worker_rooted
       dispatch_live dispatch_rooted
       batch_live batch_rooted
       late_worker_live : bool)
      : Prop :=
    forall addr,
      boundary_scheduler_live
        active_worker_live dispatch_live batch_live late_worker_live addr ->
      ~ boundary_freed
          active_worker_live worker_rooted
          dispatch_live dispatch_rooted
          batch_live batch_rooted
          late_worker_live addr.

  Theorem rooted_closed_scheduler_boundary_safe :
    forall active_worker_live dispatch_live batch_live,
      scheduler_boundary_safe
        active_worker_live active_worker_live
        dispatch_live dispatch_live
        batch_live batch_live
        false.
  Proof.
    intros active_worker_live dispatch_live batch_live.
    unfold scheduler_boundary_safe, boundary_scheduler_live, boundary_freed.
    set (ActiveRoot := boundary_root active_worker_live BoundaryActiveWorker).
    set (DispatchRoot := boundary_root dispatch_live BoundaryDispatchFanout).
    set (BatchRoot := boundary_root batch_live BoundaryBatchHandoff).
    set (NewRoot := boundary_root false BoundaryNewAdmission).
    set (DriverRoot :=
           boundary_driver_root active_worker_live dispatch_live batch_live).
    change
      (forall addr,
        @SchedulerLiveRoot BoundaryAddr ActiveRoot DispatchRoot BatchRoot
          NewRoot addr ->
        ~ (@SchedulerLiveRoot BoundaryAddr ActiveRoot DispatchRoot BatchRoot
             NewRoot addr /\ ~ DriverRoot addr)).
    eapply (@scheduler_live_root_survives_collection
      BoundaryAddr ActiveRoot DispatchRoot BatchRoot NewRoot DriverRoot
      DriverRoot
      (fun addr =>
         @SchedulerLiveRoot BoundaryAddr ActiveRoot DispatchRoot BatchRoot
           NewRoot addr /\ ~ DriverRoot addr)).
    - intros addr Hroot.
      unfold DriverRoot, boundary_driver_root.
      left.
      exact Hroot.
    - intros addr Hroot.
      unfold DriverRoot, boundary_driver_root.
      right.
      left.
      exact Hroot.
    - intros addr Hroot.
      unfold DriverRoot, boundary_driver_root.
      right.
      right.
      exact Hroot.
    - intros addr Hnew.
      unfold NewRoot, boundary_root in Hnew.
      destruct Hnew as [Hfalse _].
      discriminate Hfalse.
    - intros addr Hdriver.
      exact Hdriver.
    - intros addr Hfreed Hmarked.
      destruct Hfreed as [_ Hnot_driver].
      exact (Hnot_driver Hmarked).
  Qed.

  Theorem gc_window_safe_exports_boundary_driver_roots :
    forall active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live,
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      (forall addr,
        boundary_root active_worker_live BoundaryActiveWorker addr ->
        boundary_driver_root worker_rooted dispatch_rooted batch_rooted addr) /\
      (forall addr,
        boundary_root dispatch_live BoundaryDispatchFanout addr ->
        boundary_driver_root worker_rooted dispatch_rooted batch_rooted addr) /\
      (forall addr,
        boundary_root batch_live BoundaryBatchHandoff addr ->
        boundary_driver_root worker_rooted dispatch_rooted batch_rooted addr) /\
      (forall addr,
        boundary_root late_worker_live BoundaryNewAdmission addr ->
        False).
  Proof.
    intros [] [] [] [] [] [] [] Hsafe;
      unfold gc_window_safe, sweep_frees_live_value in Hsafe;
      simpl in Hsafe; try discriminate Hsafe;
      repeat split;
      intros addr Hroot;
      unfold boundary_driver_root, boundary_root in *;
      destruct Hroot as [Hlive ->]; try discriminate Hlive;
      auto.
  Qed.

  Theorem missing_active_worker_boundary_root_exposes_gap :
    ~ scheduler_boundary_safe true false false false false false false.
  Proof.
    intros Hsafe.
    specialize (Hsafe BoundaryActiveWorker).
    unfold boundary_scheduler_live, boundary_freed, boundary_driver_root,
      boundary_root in Hsafe.
    assert (Hlive :
      @SchedulerLiveRoot BoundaryAddr
        (fun addr : BoundaryAddr => true = true /\ addr = BoundaryActiveWorker)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryDispatchFanout)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryBatchHandoff)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryNewAdmission)
        BoundaryActiveWorker).
    { left. split; reflexivity. }
    apply (Hsafe Hlive).
    split.
    - exact Hlive.
    - intros [[Hfalse _] | [[Hfalse _] | [Hfalse _]]].
      + discriminate Hfalse.
      + discriminate Hfalse.
      + discriminate Hfalse.
  Qed.

  Theorem missing_dispatch_boundary_root_exposes_gap :
    ~ scheduler_boundary_safe false false true false false false false.
  Proof.
    intros Hsafe.
    specialize (Hsafe BoundaryDispatchFanout).
    unfold boundary_scheduler_live, boundary_freed, boundary_driver_root,
      boundary_root in Hsafe.
    assert (Hlive :
      @SchedulerLiveRoot BoundaryAddr
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryActiveWorker)
        (fun addr : BoundaryAddr => true = true /\ addr = BoundaryDispatchFanout)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryBatchHandoff)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryNewAdmission)
        BoundaryDispatchFanout).
    { right. left. split; reflexivity. }
    apply (Hsafe Hlive).
    split.
    - exact Hlive.
    - intros [[Hfalse _] | [[Hfalse _] | [Hfalse _]]].
      + discriminate Hfalse.
      + discriminate Hfalse.
      + discriminate Hfalse.
  Qed.

  Theorem missing_batch_boundary_root_exposes_gap :
    ~ scheduler_boundary_safe false false false false true false false.
  Proof.
    intros Hsafe.
    specialize (Hsafe BoundaryBatchHandoff).
    unfold boundary_scheduler_live, boundary_freed, boundary_driver_root,
      boundary_root in Hsafe.
    assert (Hlive :
      @SchedulerLiveRoot BoundaryAddr
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryActiveWorker)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryDispatchFanout)
        (fun addr : BoundaryAddr => true = true /\ addr = BoundaryBatchHandoff)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryNewAdmission)
        BoundaryBatchHandoff).
    { right. right. left. split; reflexivity. }
    apply (Hsafe Hlive).
    split.
    - exact Hlive.
    - intros [[Hfalse _] | [[Hfalse _] | [Hfalse _]]].
      + discriminate Hfalse.
      + discriminate Hfalse.
      + discriminate Hfalse.
  Qed.

  Theorem open_admission_boundary_root_exposes_gap :
    ~ scheduler_boundary_safe false false false false false false true.
  Proof.
    intros Hsafe.
    specialize (Hsafe BoundaryNewAdmission).
    unfold boundary_scheduler_live, boundary_freed, boundary_driver_root,
      boundary_root in Hsafe.
    assert (Hlive :
      @SchedulerLiveRoot BoundaryAddr
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryActiveWorker)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryDispatchFanout)
        (fun addr : BoundaryAddr => false = true /\ addr = BoundaryBatchHandoff)
        (fun addr : BoundaryAddr => true = true /\ addr = BoundaryNewAdmission)
        BoundaryNewAdmission).
    { right. right. right. split; reflexivity. }
    apply (Hsafe Hlive).
    split.
    - exact Hlive.
    - intros [[Hfalse _] | [[Hfalse _] | [Hfalse _]]].
      + discriminate Hfalse.
      + discriminate Hfalse.
      + discriminate Hfalse.
  Qed.

  Record SpawnLatchConfig : Type := {
    spawn_latch_at : nat;
    spawn_worker_at : nat;
    spawn_check_at : nat
  }.

  Definition spawn_trace (c : SpawnLatchConfig) : SpawnTrace :=
    {| latch_at := spawn_latch_at c;
       spawn_at := spawn_worker_at c;
       check_at := spawn_check_at c |}.

  Definition spawn_latch_safe (c : SpawnLatchConfig) : Prop :=
    latch_before_spawn (spawn_trace c) /\
    (worker_exists_at_check (spawn_trace c) ->
     ~ midloop_gate_open_at_check True True 1 (spawn_trace c)).

  Definition complete_spawn_latch : SpawnLatchConfig :=
    {| spawn_latch_at := 0;
       spawn_worker_at := 1;
       spawn_check_at := 2 |}.

  Definition spawn_before_latch : SpawnLatchConfig :=
    {| spawn_latch_at := 2;
       spawn_worker_at := 0;
       spawn_check_at := 1 |}.

  Theorem complete_spawn_latch_safe :
    spawn_latch_safe complete_spawn_latch.
  Proof.
    unfold spawn_latch_safe, complete_spawn_latch, spawn_trace.
    simpl.
    split.
    - unfold latch_before_spawn. simpl. lia.
    - intros Hworker.
      apply latch_before_spawn_blocks_worker_midloop_overlap.
      + unfold latch_before_spawn. simpl. lia.
      + exact Hworker.
  Qed.

  Theorem spawn_before_latch_exposes_latch_gap :
    ~ spawn_latch_safe spawn_before_latch.
  Proof.
    intros [Hbefore _].
    unfold latch_before_spawn, spawn_trace, spawn_before_latch in Hbefore.
    simpl in Hbefore.
    lia.
  Qed.

  Record CronState : Type := {
    cron_in_flight : bool;
    cron_stop_requested : bool;
    cron_overlap : bool;
    cron_dispatched_again : bool
  }.

  Definition cron_dispatch_state (s : CronState) : DispatchState :=
    {| in_flight := cron_in_flight s;
       stop_requested := cron_stop_requested s |}.

  Definition cron_first_due (claim_before_dispatch : bool) : CronState :=
    {| cron_in_flight := claim_before_dispatch;
       cron_stop_requested := false;
       cron_overlap := false;
       cron_dispatched_again := false |}.

  Definition cron_second_due (s : CronState) : CronState :=
    if cron_in_flight s then
      {| cron_in_flight := true;
         cron_stop_requested := cron_stop_requested s;
         cron_overlap := cron_overlap s;
         cron_dispatched_again := cron_dispatched_again s |}
    else
      {| cron_in_flight := false;
         cron_stop_requested := cron_stop_requested s;
         cron_overlap := true;
         cron_dispatched_again := cron_dispatched_again s |}.

  Definition cron_worker_complete
      (publish_stop_before_idle : bool)
      (s : CronState)
      : CronState :=
    let completed :=
      worker_complete
        (if publish_stop_before_idle then Stop else Continue)
        (cron_dispatch_state s) in
    {| cron_in_flight := in_flight completed;
       cron_stop_requested := stop_requested completed;
       cron_overlap := cron_overlap s;
       cron_dispatched_again := cron_dispatched_again s |}.

  Definition cron_final_due (s : CronState) : CronState :=
    match cron_due (cron_dispatch_state s) with
    | Dispatch =>
        {| cron_in_flight := cron_in_flight s;
           cron_stop_requested := cron_stop_requested s;
           cron_overlap := cron_overlap s;
           cron_dispatched_again := true |}
    | RequeueOnly | DropRecurring =>
        {| cron_in_flight := cron_in_flight s;
           cron_stop_requested := cron_stop_requested s;
           cron_overlap := cron_overlap s;
           cron_dispatched_again := false |}
    end.

  Definition cron_no_overlap (s : CronState) : Prop :=
    cron_overlap s = false.

  Definition cron_stop_prevents_redispatch (s : CronState) : Prop :=
    cron_dispatched_again s = false.

  Definition cron_dispatch_safe (s : CronState) : Prop :=
    cron_no_overlap s /\
    cron_stop_prevents_redispatch s.

  Theorem claimed_cron_dispatch_prevents_overlap :
    cron_no_overlap (cron_second_due (cron_first_due true)).
  Proof.
    reflexivity.
  Qed.

  Theorem published_stop_prevents_redispatch :
    cron_stop_prevents_redispatch
      (cron_final_due
        (cron_worker_complete true
          (cron_second_due (cron_first_due true)))).
  Proof.
    reflexivity.
  Qed.

  Theorem complete_cron_dispatch_safe :
    cron_dispatch_safe
      (cron_final_due
        (cron_worker_complete true
          (cron_second_due (cron_first_due true)))).
  Proof.
    split.
    - apply claimed_cron_dispatch_prevents_overlap.
    - apply published_stop_prevents_redispatch.
  Qed.

  Theorem unclaimed_cron_dispatch_exposes_overlap :
    cron_overlap (cron_second_due (cron_first_due false)) = true.
  Proof.
    reflexivity.
  Qed.

  Theorem missing_cron_stop_publish_exposes_redispatch :
    cron_dispatched_again
      (cron_final_due
        (cron_worker_complete false
          (cron_second_due (cron_first_due true)))) = true.
  Proof.
    reflexivity.
  Qed.

  Record WorkPoolConfig : Type := {
    work_pool_submitted : nat;
    work_pool_retained : nat;
    work_pool_inner_catch : bool;
    work_pool_outer_catch : bool;
    work_pool_failure : FailureKind;
    work_pool_overflow_requested : nat;
    work_pool_overflow_live : nat;
    work_pool_overflow_max : nat;
    work_pool_overflow_spawned : nat;
    work_pool_lifecycle_active : nat;
    work_pool_lifecycle_parked : nat;
    work_pool_lifecycle_max : nat;
    work_pool_double_unpark_active : nat;
    work_pool_double_unpark_parked : nat;
    work_pool_respawn_active : nat;
    work_pool_respawn_parked : nat;
    work_pool_recompute_at_pop : bool;
    work_pool_old_base : Z;
    work_pool_new_base : Z;
    work_pool_old_age : Z;
    work_pool_new_age : Z;
    work_pool_old_seq : nat;
    work_pool_new_seq : nat;
    work_pool_popped_old : bool
  }.

  Definition work_pool_startup_safe (c : WorkPoolConfig) : Prop :=
    work_pool_retained c = work_pool_submitted c.

  Definition work_pool_panic_safe (c : WorkPoolConfig) : Prop :=
    let r :=
      handle_failure
        (work_pool_inner_catch c)
        (work_pool_outer_catch c)
        (work_pool_failure c) in
    next_task_can_run r /\
    runtime_update_skipped r /\
    (work_pool_failure c = TaskClosurePanic ->
     task_panic_heartbeat_published r).

  Definition work_pool_overflow_safe (c : WorkPoolConfig) : Prop :=
    work_pool_overflow_live c <= work_pool_overflow_max c /\
    work_pool_overflow_spawned c =
      overflow_spawn_quota
        (work_pool_overflow_requested c)
        (work_pool_overflow_live c)
        (work_pool_overflow_max c) /\
    work_pool_overflow_live c + work_pool_overflow_spawned c
      <= work_pool_overflow_max c.

  Definition work_pool_lifecycle_safe (c : WorkPoolConfig) : Prop :=
    0 < work_pool_lifecycle_parked c /\
    consistent
      (work_pool_lifecycle_active c)
      (work_pool_lifecycle_parked c)
      (work_pool_lifecycle_max c) /\
    work_pool_double_unpark_active c =
      try_unpark_count (work_pool_lifecycle_active c) Parked /\
    work_pool_double_unpark_parked c =
      work_pool_lifecycle_parked c - 1 /\
    consistent
      (work_pool_double_unpark_active c)
      (work_pool_double_unpark_parked c)
      (work_pool_lifecycle_max c) /\
    work_pool_respawn_active c =
      respawn_unpark_count (work_pool_lifecycle_active c) Parked /\
    work_pool_respawn_parked c =
      work_pool_lifecycle_parked c - 1 /\
    consistent
      (work_pool_respawn_active c)
      (work_pool_respawn_parked c)
      (work_pool_lifecycle_max c).

  Definition work_pool_old_task (c : WorkPoolConfig) : TaskScore :=
    {| base_priority := work_pool_old_base c;
       age_ticks := work_pool_old_age c;
       sequence := work_pool_old_seq c |}.

  Definition work_pool_new_task (c : WorkPoolConfig) : TaskScore :=
    {| base_priority := work_pool_new_base c;
       age_ticks := work_pool_new_age c;
       sequence := work_pool_new_seq c |}.

  Definition work_pool_priority_fair_safe (c : WorkPoolConfig) : Prop :=
    work_pool_recompute_at_pop c = true /\
    ((work_pool_old_age c >
      work_pool_old_base c - work_pool_new_base c + work_pool_new_age c)%Z ->
     StrictlyBefore (work_pool_old_task c) (work_pool_new_task c) /\
     work_pool_popped_old c = true).

  Definition work_pool_envelope_safe (c : WorkPoolConfig) : Prop :=
    work_pool_startup_safe c /\
    work_pool_panic_safe c /\
    work_pool_overflow_safe c /\
    work_pool_lifecycle_safe c /\
    work_pool_priority_fair_safe c.

  Definition complete_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := true;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic;
       work_pool_overflow_requested := 3;
       work_pool_overflow_live := 0;
       work_pool_overflow_max := 2;
       work_pool_overflow_spawned := overflow_spawn_quota 3 0 2;
       work_pool_lifecycle_active := 1;
       work_pool_lifecycle_parked := 1;
       work_pool_lifecycle_max := 2;
       work_pool_double_unpark_active := try_unpark_count 1 Parked;
       work_pool_double_unpark_parked := 0;
       work_pool_respawn_active := respawn_unpark_count 1 Parked;
       work_pool_respawn_parked := 0;
       work_pool_recompute_at_pop := true;
       work_pool_old_base := 10%Z;
       work_pool_new_base := 0%Z;
       work_pool_old_age := 11%Z;
       work_pool_new_age := 0%Z;
       work_pool_old_seq := 0;
       work_pool_new_seq := 1;
       work_pool_popped_old := true |}.

  Definition lossy_work_pool_startup : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := 0;
       work_pool_inner_catch := true;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic;
       work_pool_overflow_requested := 3;
       work_pool_overflow_live := 0;
       work_pool_overflow_max := 2;
       work_pool_overflow_spawned := overflow_spawn_quota 3 0 2;
       work_pool_lifecycle_active := 1;
       work_pool_lifecycle_parked := 1;
       work_pool_lifecycle_max := 2;
       work_pool_double_unpark_active := try_unpark_count 1 Parked;
       work_pool_double_unpark_parked := 0;
       work_pool_respawn_active := respawn_unpark_count 1 Parked;
       work_pool_respawn_parked := 0;
       work_pool_recompute_at_pop := true;
       work_pool_old_base := 10%Z;
       work_pool_new_base := 0%Z;
       work_pool_old_age := 11%Z;
       work_pool_new_age := 0%Z;
       work_pool_old_seq := 0;
       work_pool_new_seq := 1;
       work_pool_popped_old := true |}.

  Definition task_panic_missing_inner_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := false;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic;
       work_pool_overflow_requested := 3;
       work_pool_overflow_live := 0;
       work_pool_overflow_max := 2;
       work_pool_overflow_spawned := overflow_spawn_quota 3 0 2;
       work_pool_lifecycle_active := 1;
       work_pool_lifecycle_parked := 1;
       work_pool_lifecycle_max := 2;
       work_pool_double_unpark_active := try_unpark_count 1 Parked;
       work_pool_double_unpark_parked := 0;
       work_pool_respawn_active := respawn_unpark_count 1 Parked;
       work_pool_respawn_parked := 0;
       work_pool_recompute_at_pop := true;
       work_pool_old_base := 10%Z;
       work_pool_new_base := 0%Z;
       work_pool_old_age := 11%Z;
       work_pool_new_age := 0%Z;
       work_pool_old_seq := 0;
       work_pool_new_seq := 1;
       work_pool_popped_old := true |}.

  Definition accounting_panic_missing_outer_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := true;
       work_pool_outer_catch := false;
       work_pool_failure := AccountingPanic;
       work_pool_overflow_requested := 3;
       work_pool_overflow_live := 0;
       work_pool_overflow_max := 2;
       work_pool_overflow_spawned := overflow_spawn_quota 3 0 2;
       work_pool_lifecycle_active := 1;
       work_pool_lifecycle_parked := 1;
       work_pool_lifecycle_max := 2;
       work_pool_double_unpark_active := try_unpark_count 1 Parked;
       work_pool_double_unpark_parked := 0;
       work_pool_respawn_active := respawn_unpark_count 1 Parked;
       work_pool_respawn_parked := 0;
       work_pool_recompute_at_pop := true;
       work_pool_old_base := 10%Z;
       work_pool_new_base := 0%Z;
       work_pool_old_age := 11%Z;
       work_pool_new_age := 0%Z;
       work_pool_old_seq := 0;
       work_pool_new_seq := 1;
       work_pool_popped_old := true |}.

  Definition uncapped_overflow_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := true;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic;
       work_pool_overflow_requested := 3;
       work_pool_overflow_live := 0;
       work_pool_overflow_max := 2;
       work_pool_overflow_spawned := 3;
       work_pool_lifecycle_active := 1;
       work_pool_lifecycle_parked := 1;
       work_pool_lifecycle_max := 2;
       work_pool_double_unpark_active := try_unpark_count 1 Parked;
       work_pool_double_unpark_parked := 0;
       work_pool_respawn_active := respawn_unpark_count 1 Parked;
       work_pool_respawn_parked := 0;
       work_pool_recompute_at_pop := true;
       work_pool_old_base := 10%Z;
       work_pool_new_base := 0%Z;
       work_pool_old_age := 11%Z;
       work_pool_new_age := 0%Z;
       work_pool_old_seq := 0;
       work_pool_new_seq := 1;
       work_pool_popped_old := true |}.

  Definition double_unpark_overcounts_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := true;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic;
       work_pool_overflow_requested := 3;
       work_pool_overflow_live := 0;
       work_pool_overflow_max := 2;
       work_pool_overflow_spawned := overflow_spawn_quota 3 0 2;
       work_pool_lifecycle_active := 1;
       work_pool_lifecycle_parked := 1;
       work_pool_lifecycle_max := 2;
       work_pool_double_unpark_active := 3;
       work_pool_double_unpark_parked := 0;
       work_pool_respawn_active := respawn_unpark_count 1 Parked;
       work_pool_respawn_parked := 0;
       work_pool_recompute_at_pop := true;
       work_pool_old_base := 10%Z;
       work_pool_new_base := 0%Z;
       work_pool_old_age := 11%Z;
       work_pool_new_age := 0%Z;
       work_pool_old_seq := 0;
       work_pool_new_seq := 1;
       work_pool_popped_old := true |}.

  Definition respawn_without_increment_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := true;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic;
       work_pool_overflow_requested := 3;
       work_pool_overflow_live := 0;
       work_pool_overflow_max := 2;
       work_pool_overflow_spawned := overflow_spawn_quota 3 0 2;
       work_pool_lifecycle_active := 1;
       work_pool_lifecycle_parked := 1;
       work_pool_lifecycle_max := 2;
       work_pool_double_unpark_active := try_unpark_count 1 Parked;
       work_pool_double_unpark_parked := 0;
       work_pool_respawn_active := 1;
       work_pool_respawn_parked := 0;
       work_pool_recompute_at_pop := true;
       work_pool_old_base := 10%Z;
       work_pool_new_base := 0%Z;
       work_pool_old_age := 11%Z;
       work_pool_new_age := 0%Z;
       work_pool_old_seq := 0;
       work_pool_new_seq := 1;
       work_pool_popped_old := true |}.

  Definition stale_priority_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := true;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic;
       work_pool_overflow_requested := 3;
       work_pool_overflow_live := 0;
       work_pool_overflow_max := 2;
       work_pool_overflow_spawned := overflow_spawn_quota 3 0 2;
       work_pool_lifecycle_active := 1;
       work_pool_lifecycle_parked := 1;
       work_pool_lifecycle_max := 2;
       work_pool_double_unpark_active := try_unpark_count 1 Parked;
       work_pool_double_unpark_parked := 0;
       work_pool_respawn_active := respawn_unpark_count 1 Parked;
       work_pool_respawn_parked := 0;
       work_pool_recompute_at_pop := false;
       work_pool_old_base := 10%Z;
       work_pool_new_base := 0%Z;
       work_pool_old_age := 11%Z;
       work_pool_new_age := 0%Z;
       work_pool_old_seq := 0;
       work_pool_new_seq := 1;
       work_pool_popped_old := false |}.

  Theorem complete_work_pool_envelope_safe :
    work_pool_envelope_safe complete_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_startup_safe,
      work_pool_panic_safe, work_pool_overflow_safe,
      work_pool_lifecycle_safe, complete_work_pool,
      prestart_queue_after_submissions.
    simpl.
    unfold consistent.
    repeat split; try reflexivity; try lia;
      try (intros Hage;
           split;
           [ apply older_task_eventually_preempts; exact Hage
           | reflexivity ]);
      try (apply older_task_eventually_preempts; lia).
  Qed.

  Theorem lossy_work_pool_startup_exposes_envelope_gap :
    ~ work_pool_envelope_safe lossy_work_pool_startup.
  Proof.
    unfold work_pool_envelope_safe, work_pool_startup_safe,
      lossy_work_pool_startup.
    simpl.
    intros [Hretained _].
    discriminate Hretained.
  Qed.

  Theorem missing_inner_task_panic_exposes_envelope_gap :
    ~ work_pool_envelope_safe task_panic_missing_inner_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_panic_safe,
      task_panic_missing_inner_work_pool.
    simpl.
    intros [_ [[_ [_ Hheartbeat]] _]].
    specialize (Hheartbeat eq_refl).
    exact (missing_inner_catch_loses_task_panic_heartbeat Hheartbeat).
  Qed.

  Theorem missing_outer_accounting_panic_exposes_envelope_gap :
    ~ work_pool_envelope_safe accounting_panic_missing_outer_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_panic_safe,
      accounting_panic_missing_outer_work_pool.
    simpl.
    intros [_ [[Halive _] _]].
    exact (missing_outer_catch_can_kill_worker_on_accounting_panic Halive).
  Qed.

  Theorem uncapped_overflow_exposes_envelope_gap :
    ~ work_pool_envelope_safe uncapped_overflow_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_overflow_safe,
      uncapped_overflow_work_pool.
    simpl.
    intros [_ [_ [[_ [_ Hcap]] _]]].
    lia.
  Qed.

  Theorem double_unpark_overcount_exposes_envelope_gap :
    ~ work_pool_envelope_safe double_unpark_overcounts_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_lifecycle_safe,
      double_unpark_overcounts_work_pool, consistent.
    simpl.
    intros [_ [_ [_ [Hlife _]]]].
    destruct Hlife as [_ [_ [_ [_ [Hconsistent _]]]]].
    lia.
  Qed.

  Theorem respawn_without_increment_exposes_envelope_gap :
    ~ work_pool_envelope_safe respawn_without_increment_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_lifecycle_safe,
      respawn_without_increment_work_pool, consistent.
    simpl.
    intros [_ [_ [_ [Hlife _]]]].
    destruct Hlife as [_ [_ [_ [_ [_ [_ [_ Hconsistent]]]]]]].
    lia.
  Qed.

  Theorem stale_priority_work_pool_exposes_envelope_gap :
    ~ work_pool_envelope_safe stale_priority_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_priority_fair_safe,
      stale_priority_work_pool.
    simpl.
    intros [_ [_ [_ [_ [Hrecompute _]]]]].
    discriminate Hrecompute.
  Qed.

  Record ActiveFanoutConfig : Type := {
    active_branch_count : nat;
    active_min_branches : nat;
    active_parallelism_degree : nat;
    active_cost_class : CostClass;
    active_max_parallel : nat;
    active_budget_granted : nat;
    active_pure : bool;
    active_dynamic_eval_gate : bool;
    active_dynamic_eval : bool;
    active_state_mutation : bool;
    active_strict_print : bool;
    active_io : bool;
    active_depth_ok : bool;
    active_pool_ok : bool;
    active_dispatched_count : nat
  }.

  Definition active_fanout_gate_safe (c : ActiveFanoutConfig) : Prop :=
    active_fanout_allowed
      (active_branch_count c)
      (active_min_branches c)
      (active_parallelism_degree c)
      (active_budget_granted c)
      (active_pure c = true)
      (active_depth_ok c = true)
      (active_pool_ok c = true) /\
    complete_dispatch (active_branch_count c) (active_dispatched_count c).

  Definition active_transducer_degree_matches (c : ActiveFanoutConfig) : Prop :=
    active_parallelism_degree c =
      branch_degree
        (active_cost_class c)
        (active_branch_count c)
        (active_max_parallel c).

  Definition active_transducer_maximal_before_cap
      (c : ActiveFanoutConfig)
      : Prop :=
    branch_parallel_class (active_cost_class c) ->
    1 < active_branch_count c ->
    active_branch_count c <= safe_cap (active_max_parallel c) ->
    active_parallelism_degree c = active_branch_count c.

  Definition active_transducer_default_gate_sound
      (c : ActiveFanoutConfig)
      : Prop :=
    1 < active_parallelism_degree c ->
    branch_parallel_class (active_cost_class c).

  Definition active_transducer_safe (c : ActiveFanoutConfig) : Prop :=
    active_transducer_degree_matches c /\
    active_transducer_maximal_before_cap c /\
    active_transducer_default_gate_sound c.

  Definition active_blocks_parallel_dispatch
      (c : ActiveFanoutConfig)
      : Prop :=
    blocks_parallel_dispatch
      (active_dynamic_eval_gate c = true)
      (active_dynamic_eval c = true)
      (active_state_mutation c = true)
      (active_strict_print c = true)
      (active_io c = true).

  Definition active_no_budget_parallel_safe
      (c : ActiveFanoutConfig)
      : Prop :=
    active_pure c = true ->
    no_budget_parallel_allowed
      (active_dynamic_eval_gate c = true)
      (active_dynamic_eval c = true)
      (active_state_mutation c = true)
      (active_strict_print c = true)
      (active_io c = true).

  Definition active_parallel_dispatch_blockers_safe
      (c : ActiveFanoutConfig)
      : Prop :=
    (active_dynamic_eval c = true -> active_blocks_parallel_dispatch c) /\
    (active_state_mutation c = true -> active_blocks_parallel_dispatch c) /\
    (active_strict_print c = true ->
	 active_io c = true ->
	 active_blocks_parallel_dispatch c) /\
    active_no_budget_parallel_safe c.

  Definition active_fanout_admitted_by_standalone
      (c : ActiveFanoutConfig)
      : Prop :=
    FanoutAdmission.fanout_admitted
      (active_branch_count c)
      (active_min_branches c)
      (active_parallelism_degree c)
      (active_budget_granted c)
      (active_pure c = true)
      (active_depth_ok c = true)
      (active_pool_ok c = true).

  Definition active_fanout_admission_represents_every_branch
      (c : ActiveFanoutConfig)
      : Prop :=
    forall slot,
      slot < active_branch_count c ->
      FanoutAdmission.branch_slot_represented
        (active_dispatched_count c)
        slot.

  Definition active_collapse_admitted_by_standalone
      (c : ActiveFanoutConfig)
      : Prop :=
    CollapseAdmission.collapse_admitted
      (active_branch_count c)
      (active_min_branches c)
      (active_budget_granted c)
      (active_depth_ok c = true)
      (active_pool_ok c = true).

  Definition active_collapse_admission_represents_every_item
      (c : ActiveFanoutConfig)
      : Prop :=
    forall slot,
      slot < active_branch_count c ->
      CollapseAdmission.item_slot_represented
        (active_dispatched_count c)
        slot.

  Definition active_fanout_admission_complete
      (c : ActiveFanoutConfig)
      : Prop :=
    active_fanout_admission_represents_every_branch c /\
    active_collapse_admission_represents_every_item c.

  Definition active_fanout_stack_safe (c : ActiveFanoutConfig) : Prop :=
    active_fanout_gate_safe c /\
    active_transducer_safe c /\
    active_parallel_dispatch_blockers_safe c /\
    active_fanout_admission_complete c.

  Definition active_fanout_envelope_safe
      (w : Workload)
      (c : ActiveFanoutConfig)
      : Prop :=
    uses_direct_fanout w = true -> active_fanout_stack_safe c.

  Definition complete_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 2;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := ParallelPure;
       active_max_parallel := 2;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Definition missing_purity_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 2;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := ParallelPure;
       active_max_parallel := 2;
       active_budget_granted := 1;
       active_pure := false;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Definition missing_budget_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 2;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := ParallelPure;
       active_max_parallel := 2;
       active_budget_granted := 0;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Definition partial_dispatch_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 2;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := ParallelPure;
       active_max_parallel := 2;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 1 |}.

  Definition degree_capped_admission_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 3;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := ParallelPure;
       active_max_parallel := 3;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Definition threshold_capped_collapse_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 3;
       active_min_branches := 2;
       active_parallelism_degree := 3;
       active_cost_class := ParallelPure;
       active_max_parallel := 3;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Definition zero_cap_bug_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 4;
       active_min_branches := 2;
       active_parallelism_degree := 0;
       active_cost_class := SymbolicModerate;
       active_max_parallel := 0;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 4 |}.

  Definition underutilized_transducer_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 4;
       active_min_branches := 2;
       active_parallelism_degree := 3;
       active_cost_class := ParallelPure;
       active_max_parallel := 8;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 4 |}.

  Definition non_branch_parallel_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 2;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := GroundCheap;
       active_max_parallel := 2;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Definition missing_dynamic_eval_gate_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 2;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := ParallelPure;
       active_max_parallel := 2;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := false;
       active_dynamic_eval := true;
       active_state_mutation := false;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Definition state_mutation_bypass_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 2;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := ParallelPure;
       active_max_parallel := 2;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := true;
       active_strict_print := false;
       active_io := false;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Definition strict_io_bypass_active_fanout : ActiveFanoutConfig :=
    {| active_branch_count := 2;
       active_min_branches := 2;
       active_parallelism_degree := 2;
       active_cost_class := ParallelPure;
       active_max_parallel := 2;
       active_budget_granted := 1;
       active_pure := true;
       active_dynamic_eval_gate := true;
       active_dynamic_eval := false;
       active_state_mutation := false;
       active_strict_print := true;
       active_io := true;
       active_depth_ok := true;
       active_pool_ok := true;
       active_dispatched_count := 2 |}.

  Theorem complete_active_fanout_gate_safe :
    active_fanout_gate_safe complete_active_fanout.
  Proof.
    unfold active_fanout_gate_safe, complete_active_fanout,
      active_fanout_allowed, degree_gate, budget_gate, complete_dispatch.
    simpl.
    repeat split; lia.
  Qed.

  Theorem active_fanout_gate_safe_implies_standalone_admission :
    forall c,
      active_fanout_gate_safe c ->
      active_fanout_admitted_by_standalone c.
  Proof.
    intros c [Hallowed _].
    unfold active_fanout_admitted_by_standalone.
    unfold active_fanout_allowed in Hallowed.
    unfold FanoutAdmission.fanout_admitted,
      FanoutAdmission.degree_gate.
    unfold degree_gate, budget_gate in Hallowed.
    destruct Hallowed as
      [Hmin [Hdegree [Hpure [Hdepth [Hpool Hbudget]]]]].
    repeat split; assumption.
  Qed.

  Theorem active_fanout_gate_safe_implies_collapse_admission :
    forall c,
      active_fanout_gate_safe c ->
      active_collapse_admitted_by_standalone c.
  Proof.
    intros c [Hallowed _].
    unfold active_collapse_admitted_by_standalone.
    unfold active_fanout_allowed in Hallowed.
    unfold CollapseAdmission.collapse_admitted,
      CollapseAdmission.threshold_gate.
    unfold degree_gate, budget_gate in Hallowed.
    destruct Hallowed as
      [Hmin [_ [_ [Hdepth [Hpool Hbudget]]]]].
    repeat split; assumption.
  Qed.

  Theorem active_fanout_gate_safe_represents_admitted_branches :
    forall c,
      active_fanout_gate_safe c ->
      active_fanout_admission_represents_every_branch c.
  Proof.
    intros c Hgate slot Hslot.
    unfold active_fanout_admission_represents_every_branch.
    apply
      (FanoutAdmission.admitted_complete_fanout_represents_every_branch
        (active_branch_count c)
        (active_min_branches c)
        (active_parallelism_degree c)
        (active_budget_granted c)
        (active_pure c = true)
        (active_depth_ok c = true)
        (active_pool_ok c = true)
        (active_dispatched_count c)).
    - apply active_fanout_gate_safe_implies_standalone_admission.
      exact Hgate.
    - destruct Hgate as [_ Hcomplete].
      unfold FanoutAdmission.complete_dispatch.
      unfold complete_dispatch in Hcomplete.
      exact Hcomplete.
    - exact Hslot.
  Qed.

  Theorem active_fanout_gate_safe_represents_collapse_items :
    forall c,
      active_fanout_gate_safe c ->
      active_collapse_admission_represents_every_item c.
  Proof.
    intros c Hgate slot Hslot.
    unfold active_collapse_admission_represents_every_item.
    apply
      (CollapseAdmission.admitted_complete_collapse_represents_every_item
        (active_branch_count c)
        (active_min_branches c)
        (active_budget_granted c)
        (active_depth_ok c = true)
        (active_pool_ok c = true)
        (active_dispatched_count c)).
    - apply active_fanout_gate_safe_implies_collapse_admission.
      exact Hgate.
    - destruct Hgate as [_ Hcomplete].
      unfold CollapseAdmission.complete_dispatch.
      unfold complete_dispatch in Hcomplete.
      exact Hcomplete.
    - exact Hslot.
  Qed.

  Theorem active_fanout_gate_safe_implies_admission_complete :
    forall c,
      active_fanout_gate_safe c ->
      active_fanout_admission_complete c.
  Proof.
    intros c Hgate.
    split.
    - apply active_fanout_gate_safe_represents_admitted_branches.
      exact Hgate.
    - apply active_fanout_gate_safe_represents_collapse_items.
      exact Hgate.
  Qed.

  Theorem complete_active_transducer_safe :
    active_transducer_safe complete_active_fanout.
  Proof.
    unfold active_transducer_safe, active_transducer_degree_matches,
      active_transducer_maximal_before_cap,
      active_transducer_default_gate_sound, complete_active_fanout.
    simpl.
    repeat split; try reflexivity; intros; exact I.
  Qed.

  Theorem complete_active_fanout_stack_safe :
    active_fanout_stack_safe complete_active_fanout.
  Proof.
    split.
    - apply complete_active_fanout_gate_safe.
    - split.
      + apply complete_active_transducer_safe.
      + split.
        * repeat split.
          -- intros Hdyn; discriminate Hdyn.
          -- intros Hmutation; discriminate Hmutation.
          -- intros Hstrict _; discriminate Hstrict.
          -- unfold active_no_budget_parallel_safe, no_budget_parallel_allowed,
              blocks_parallel_dispatch, complete_active_fanout.
             simpl.
             intros _ [Hstate | [[Hstrict _] | [_ Hdyn]]].
             ++ discriminate Hstate.
             ++ discriminate Hstrict.
             ++ discriminate Hdyn.
        * apply active_fanout_gate_safe_implies_admission_complete.
          apply complete_active_fanout_gate_safe.
  Qed.

  Theorem missing_purity_active_fanout_exposes_envelope_gap :
    ~ active_fanout_gate_safe missing_purity_active_fanout.
  Proof.
    intros [Hallowed _].
    unfold missing_purity_active_fanout in Hallowed.
    simpl in Hallowed.
    unfold active_fanout_allowed in Hallowed.
    destruct Hallowed as [_ [_ [Hpure _]]].
    discriminate Hpure.
  Qed.

  Theorem missing_budget_active_fanout_exposes_envelope_gap :
    ~ active_fanout_gate_safe missing_budget_active_fanout.
  Proof.
    intros [Hallowed _].
    unfold missing_budget_active_fanout in Hallowed.
    simpl in Hallowed.
    unfold active_fanout_allowed, budget_gate in Hallowed.
    destruct Hallowed as [_ [_ [_ [_ [_ Hbudget]]]]].
    lia.
  Qed.

  Theorem partial_dispatch_active_fanout_exposes_envelope_gap :
    ~ active_fanout_gate_safe partial_dispatch_active_fanout.
  Proof.
    intros [_ Hcomplete].
    unfold partial_dispatch_active_fanout, complete_dispatch in Hcomplete.
    simpl in Hcomplete.
    discriminate Hcomplete.
  Qed.

  Theorem degree_capped_active_fanout_exposes_admission_gap :
    ~ active_fanout_admission_represents_every_branch
        degree_capped_admission_active_fanout.
  Proof.
    intros Hall.
    pose proof
      (FanoutAdmission.degree_capped_partial_dispatch_misses_slot
        3 2 ltac:(lia) ltac:(lia))
      as [slot [Hslot Hmiss]].
    apply Hmiss.
    specialize (Hall slot Hslot).
    unfold degree_capped_admission_active_fanout in Hall.
    simpl in Hall.
    exact Hall.
  Qed.

  Theorem threshold_capped_collapse_exposes_admission_gap :
    ~ active_collapse_admission_represents_every_item
        threshold_capped_collapse_active_fanout.
  Proof.
    intros Hall.
    pose proof
      (CollapseAdmission.threshold_capped_partial_dispatch_misses_slot
        3 2 ltac:(lia))
      as [slot [Hslot Hmiss]].
    apply Hmiss.
    specialize (Hall slot Hslot).
    unfold threshold_capped_collapse_active_fanout in Hall.
    simpl in Hall.
    exact Hall.
  Qed.

  Theorem degree_capped_active_fanout_exposes_combined_admission_gap :
    ~ active_fanout_admission_complete degree_capped_admission_active_fanout.
  Proof.
    intros [Hbranches _].
    exact (degree_capped_active_fanout_exposes_admission_gap Hbranches).
  Qed.

  Theorem threshold_capped_collapse_exposes_combined_admission_gap :
    ~ active_fanout_admission_complete threshold_capped_collapse_active_fanout.
  Proof.
    intros [_ Hcollapse].
    exact (threshold_capped_collapse_exposes_admission_gap Hcollapse).
  Qed.

  Theorem zero_cap_bug_active_fanout_exposes_envelope_gap :
    ~ active_fanout_stack_safe zero_cap_bug_active_fanout.
  Proof.
    intros [_ [[Hmatches _] _]].
    unfold active_transducer_degree_matches, zero_cap_bug_active_fanout in
      Hmatches.
    simpl in Hmatches.
    discriminate Hmatches.
  Qed.

  Theorem underutilized_transducer_exposes_envelope_gap :
    ~ active_fanout_stack_safe underutilized_transducer_active_fanout.
  Proof.
    intros [_ [[_ [Hmaximal _]] _]].
    unfold active_transducer_maximal_before_cap,
      underutilized_transducer_active_fanout in Hmaximal.
    simpl in Hmaximal.
    assert (Hbranches : 1 < 4) by lia.
    assert (Hcap : 4 <= safe_cap 8) by
      (unfold safe_cap; lia).
    specialize (Hmaximal I Hbranches Hcap).
    lia.
  Qed.

  Theorem non_branch_parallel_class_exposes_envelope_gap :
    ~ active_fanout_stack_safe non_branch_parallel_active_fanout.
  Proof.
    intros [_ [[_ [_ Hgate]] _]].
    unfold active_transducer_default_gate_sound,
      non_branch_parallel_active_fanout in Hgate.
    simpl in Hgate.
    exact (Hgate ltac:(lia)).
  Qed.

  Theorem missing_dynamic_eval_gate_exposes_envelope_gap :
    ~ active_fanout_stack_safe missing_dynamic_eval_gate_active_fanout.
  Proof.
    intros [_ [_ [[Hdynamic _] _]]].
    unfold active_blocks_parallel_dispatch,
      missing_dynamic_eval_gate_active_fanout in Hdynamic.
    simpl in Hdynamic.
    specialize (Hdynamic eq_refl).
    unfold blocks_parallel_dispatch in Hdynamic.
    destruct Hdynamic as [Hstate | [[Hstrict _] | [Hgate _]]].
    - discriminate Hstate.
    - discriminate Hstrict.
    - discriminate Hgate.
  Qed.

  Theorem state_mutation_bypass_exposes_envelope_gap :
    ~ active_fanout_stack_safe state_mutation_bypass_active_fanout.
  Proof.
    intros [_ [_ [Hblockers _]]].
    destruct Hblockers as [_ [_ [_ Hno_budget]]].
    unfold active_no_budget_parallel_safe, no_budget_parallel_allowed,
      blocks_parallel_dispatch, state_mutation_bypass_active_fanout in
      Hno_budget.
    simpl in Hno_budget.
    specialize (Hno_budget eq_refl).
    apply Hno_budget.
    left.
    reflexivity.
  Qed.

  Theorem strict_io_bypass_exposes_envelope_gap :
    ~ active_fanout_stack_safe strict_io_bypass_active_fanout.
  Proof.
    intros [_ [_ [Hblockers _]]].
    destruct Hblockers as [_ [_ [_ Hno_budget]]].
    unfold active_no_budget_parallel_safe, no_budget_parallel_allowed,
      blocks_parallel_dispatch, strict_io_bypass_active_fanout in
      Hno_budget.
    simpl in Hno_budget.
    specialize (Hno_budget eq_refl).
    apply Hno_budget.
    right.
    left.
    split; reflexivity.
  Qed.

  Record FanoutProgressConfig : Type := {
    fanout_active : Task -> Prop;
    fanout_contributed : Task -> Prop;
    fanout_parked : Task -> Prop;
    fanout_finished : Task -> Prop;
    fanout_resumed : Task -> Prop;
    fanout_spawned : Task -> Prop;
    fanout_dropped : Task -> Prop;
    fanout_exit : Task -> ExitPath;
    fanout_trigger_sent : Prop;
    fanout_trigger_failed : Prop;
    fanout_driver_posted : Prop;
    fanout_request_cleared : Prop;
    fanout_backstop_resumed : Prop;
    fanout_cycle_closed : Prop;
    fanout_generation_advanced : Prop;
    fanout_satb_abort : Prop;
    fanout_stw_fallback_ran : Prop
  }.

  Definition fanout_exit_event
      (c : FanoutProgressConfig)
      (w : Task)
      (e : ExitPath)
      : Prop :=
    fanout_exit c w = e.

  Definition fanout_progress_safe
      (c : FanoutProgressConfig)
      : Prop :=
    (forall w,
        fanout_active c w ->
        @ParticipantAccounted Task
          (fanout_parked c)
          (fanout_finished c)
          w) /\
    (forall w,
        fanout_active c w ->
        fanout_parked c w ->
        fanout_resumed c w) /\
    ~ @ParentWaitStranded Task
        (fanout_spawned c)
        (fanout_dropped c).

  Definition all_tasks (_ : Task) : Prop := True.

  Definition no_tasks (_ : Task) : Prop := False.

  Definition complete_fanout_progress : FanoutProgressConfig :=
    {| fanout_active := all_tasks;
       fanout_contributed := all_tasks;
       fanout_parked := all_tasks;
       fanout_finished := all_tasks;
       fanout_resumed := all_tasks;
       fanout_spawned := all_tasks;
       fanout_dropped := all_tasks;
       fanout_exit := fun _ => NormalExit;
       fanout_trigger_sent := True;
       fanout_trigger_failed := False;
       fanout_driver_posted := True;
       fanout_request_cleared := True;
       fanout_backstop_resumed := True;
       fanout_cycle_closed := True;
       fanout_generation_advanced := True;
       fanout_satb_abort := False;
       fanout_stw_fallback_ran := True |}.

  Definition missing_parked_resume_fanout : FanoutProgressConfig :=
    {| fanout_active := all_tasks;
       fanout_contributed := all_tasks;
       fanout_parked := all_tasks;
       fanout_finished := all_tasks;
       fanout_resumed := no_tasks;
       fanout_spawned := all_tasks;
       fanout_dropped := all_tasks;
       fanout_exit := fun _ => NormalExit;
       fanout_trigger_sent := True;
       fanout_trigger_failed := False;
       fanout_driver_posted := True;
       fanout_request_cleared := True;
       fanout_backstop_resumed := True;
       fanout_cycle_closed := True;
       fanout_generation_advanced := True;
       fanout_satb_abort := False;
       fanout_stw_fallback_ran := True |}.

  Definition missing_participant_accounting_fanout : FanoutProgressConfig :=
    {| fanout_active := all_tasks;
       fanout_contributed := no_tasks;
       fanout_parked := no_tasks;
       fanout_finished := no_tasks;
       fanout_resumed := all_tasks;
       fanout_spawned := all_tasks;
       fanout_dropped := all_tasks;
       fanout_exit := fun _ => NormalExit;
       fanout_trigger_sent := True;
       fanout_trigger_failed := False;
       fanout_driver_posted := True;
       fanout_request_cleared := True;
       fanout_backstop_resumed := True;
       fanout_cycle_closed := True;
       fanout_generation_advanced := True;
       fanout_satb_abort := False;
       fanout_stw_fallback_ran := True |}.

  Definition missing_completion_drop_fanout : FanoutProgressConfig :=
    {| fanout_active := all_tasks;
       fanout_contributed := all_tasks;
       fanout_parked := all_tasks;
       fanout_finished := all_tasks;
       fanout_resumed := all_tasks;
       fanout_spawned := all_tasks;
       fanout_dropped := no_tasks;
       fanout_exit := fun _ => NormalExit;
       fanout_trigger_sent := True;
       fanout_trigger_failed := False;
       fanout_driver_posted := True;
       fanout_request_cleared := True;
       fanout_backstop_resumed := True;
       fanout_cycle_closed := True;
       fanout_generation_advanced := True;
       fanout_satb_abort := False;
       fanout_stw_fallback_ran := True |}.

  Theorem scheduler_fanout_contract_implies_e2e_progress :
    forall c,
      @AllParticipantsContributed Task
        (fanout_active c)
        (fanout_contributed c) ->
      (forall w,
        fanout_contributed c w ->
        @ParticipantAccounted Task
          (fanout_parked c)
          (fanout_finished c)
          w) ->
      SuccessfulTriggerHasDriver
        (fanout_trigger_sent c)
        (fanout_driver_posted c) ->
      TriggerFailureBackstopped
        (fanout_trigger_failed c)
        (fanout_request_cleared c)
        (fanout_backstop_resumed c) ->
      (fanout_backstop_resumed c ->
       forall w,
        fanout_active c w ->
        fanout_parked c w ->
        fanout_resumed c w) ->
      DriverCompletesOrAborts
        (fanout_driver_posted c)
        (fanout_cycle_closed c)
        (fanout_satb_abort c) ->
      (fanout_satb_abort c -> fanout_stw_fallback_ran c) ->
      (fanout_stw_fallback_ran c -> fanout_cycle_closed c) ->
      (fanout_cycle_closed c -> fanout_generation_advanced c) ->
      (forall w,
        fanout_active c w ->
        fanout_parked c w ->
        fanout_generation_advanced c ->
        fanout_resumed c w) ->
      (forall w,
        fanout_spawned c w ->
        @WorkerExited Task (fanout_exit_event c) w) ->
      (forall w,
        fanout_exit_event c w NormalExit ->
        fanout_dropped c w) ->
      (forall w,
        fanout_exit_event c w PanicExit ->
        fanout_dropped c w) ->
      (fanout_trigger_sent c \/ fanout_trigger_failed c) ->
      fanout_progress_safe c.
  Proof.
    intros c Hall Haccounted Hsent_driver Hfailed_backstop Hbackstop_resumes
      Hdriver_progress Habort_fallback Hfallback_closes Hclosed_gen
      Hgeneration_resumes Hexits Hnormal Hpanic Htrigger.
    unfold fanout_progress_safe.
    eapply (@scheduler_fanout_gc_progress_contract Task).
    - exact Hall.
    - exact Haccounted.
    - exact Hsent_driver.
    - exact Hfailed_backstop.
    - exact Hbackstop_resumes.
    - exact Hdriver_progress.
    - exact Habort_fallback.
    - exact Hfallback_closes.
    - exact Hclosed_gen.
    - exact Hgeneration_resumes.
    - exact Hexits.
    - exact Hnormal.
    - exact Hpanic.
    - exact Htrigger.
  Qed.

  Theorem complete_fanout_progress_safe :
    fanout_progress_safe complete_fanout_progress.
  Proof.
    unfold fanout_progress_safe, complete_fanout_progress, all_tasks.
    simpl.
    split.
    - intros w _.
      left.
      exact I.
    - split.
      + intros w _ _.
        exact I.
      + intros [w [_ Hnot_dropped]].
        apply Hnot_dropped.
        exact I.
  Qed.

  Theorem missing_participant_accounting_exposes_fanout_progress_gap :
    ~ fanout_progress_safe missing_participant_accounting_fanout.
  Proof.
    intros [Haccounted _].
    unfold missing_participant_accounting_fanout, all_tasks, no_tasks in
      Haccounted.
    simpl in Haccounted.
    destruct (Haccounted Producer I) as [Hparked | Hfinished].
    - exact Hparked.
    - exact Hfinished.
  Qed.

  Theorem missing_parked_resume_exposes_fanout_progress_gap :
    ~ fanout_progress_safe missing_parked_resume_fanout.
  Proof.
    intros [_ [Hresumed _]].
    unfold missing_parked_resume_fanout, all_tasks, no_tasks in Hresumed.
    simpl in Hresumed.
    exact (Hresumed Producer I I).
  Qed.

  Theorem missing_completion_drop_exposes_fanout_progress_gap :
    ~ fanout_progress_safe missing_completion_drop_fanout.
  Proof.
    intros [_ [_ Hnot_stranded]].
    apply Hnot_stranded.
    exists Producer.
    unfold missing_completion_drop_fanout, all_tasks, no_tasks.
    simpl.
    split.
    - exact I.
    - intros Hdrop.
      exact Hdrop.
  Qed.

  Definition end_to_end_safe
      (w : Workload)
      (wave : WaveAssignment)
      (active_worker_live worker_rooted
       dispatch_live dispatch_rooted
       batch_live batch_rooted
       late_worker_live : bool)
      (spawn_latch : SpawnLatchConfig)
      (cron_state : CronState)
      (cron_startup : StartupConfig)
      (work_pool : WorkPoolConfig)
      (active_fanout : ActiveFanoutConfig)
      (fanout_progress : FanoutProgressConfig)
      (classification_lookup : ClassificationLookupConfig)
      (e1_default_flip : E1DefaultFlipConfig)
      (e1_satb_stw_driver : E1SatbStwDriverConfig)
      (dedicated_handoff : DedicatedHandoffConfig)
      (driver_channel : DriverChannelConfig)
      : Prop :=
    schedule_envelope_safe w wave /\
    gc_window_safe
      active_worker_live worker_rooted
      dispatch_live dispatch_rooted
      batch_live batch_rooted
      late_worker_live /\
    spawn_latch_safe spawn_latch /\
    cron_dispatch_safe cron_state /\
    startup_delivery_safe cron_startup /\
    work_pool_envelope_safe work_pool /\
    active_fanout_envelope_safe w active_fanout /\
    fanout_progress_safe fanout_progress /\
    scheduler_boundary_safe
      active_worker_live worker_rooted
      dispatch_live dispatch_rooted
      batch_live batch_rooted
      late_worker_live /\
    classification_lookup_safe classification_lookup /\
    e1_default_flip_safe e1_default_flip /\
    e1_satb_stw_driver_safe e1_satb_stw_driver /\
    dedicated_handoff_safe dedicated_handoff /\
    driver_channel_protocol_safe driver_channel.

  Theorem end_to_end_safe_implies_gc_window_safe :
    forall w wave
      active_worker_live worker_rooted
      dispatch_live dispatch_rooted
      batch_live batch_rooted
      late_worker_live
      spawn_latch cron_state cron_startup work_pool active_fanout
      fanout_progress classification_lookup e1_default_flip
      e1_satb_stw_driver dedicated_handoff driver_channel,
      end_to_end_safe
        w wave
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live
        spawn_latch cron_state cron_startup work_pool active_fanout
        fanout_progress classification_lookup e1_default_flip
        e1_satb_stw_driver dedicated_handoff driver_channel ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live spawn_latch cron_state
      cron_startup work_pool active_fanout fanout_progress
      classification_lookup e1_default_flip e1_satb_stw_driver
      dedicated_handoff driver_channel Hend.
    unfold end_to_end_safe in Hend.
    exact (proj1 (proj2 Hend)).
  Qed.

  Theorem end_to_end_safe_feeds_cesk_index_gc_safety :
    forall w wave
      active_worker_live worker_rooted
      dispatch_live dispatch_rooted
      batch_live batch_rooted
      late_worker_live
      spawn_latch cron_state cron_startup work_pool active_fanout
      fanout_progress classification_lookup e1_default_flip
      e1_satb_stw_driver dedicated_handoff driver_channel
      (StructuralRoot WorkerRoot SafepointRoot EnvAnchor DispatchAnchor
       InitialRoot ShadedDeletion AllocateBlack PublishedAlloc SegmentWritten
       SegmentPublished SlotWritten SlotPublished AddrReturned ReadObserved
       ConcurrentReturned ReuseReturned Fresh OnFreeList Marked Freed
       FutureTouch : BoundaryAddr -> Prop)
      (InlineEdge SideEdge SpaceEdge ReaderEdge SatbEdge :
        BoundaryAddr -> BoundaryAddr -> Prop),
      end_to_end_safe
        w wave
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live
        spawn_latch cron_state cron_startup work_pool active_fanout
        fanout_progress classification_lookup e1_default_flip
        e1_satb_stw_driver dedicated_handoff driver_channel ->
      (forall a,
          FutureTouch a ->
          @Collector.Reach BoundaryAddr
            (@Collector.CollectorRoot BoundaryAddr StructuralRoot
              (boundary_driver_root worker_rooted dispatch_rooted batch_rooted))
            (@Collector.SemanticNodeEdge BoundaryAddr InlineEdge SideEdge
              SpaceEdge) a \/
          @Collector.DriverRootUnion BoundaryAddr WorkerRoot SafepointRoot EnvAnchor
            DispatchAnchor a \/
          @Collector.SchedulerLiveRoot BoundaryAddr
            (boundary_root active_worker_live BoundaryActiveWorker)
            (boundary_root dispatch_live BoundaryDispatchFanout)
            (boundary_root batch_live BoundaryBatchHandoff)
            (boundary_root late_worker_live BoundaryNewAdmission) a \/
          @Collector.Reach BoundaryAddr
            (@Collector.ConcurrentCollectorRoot BoundaryAddr InitialRoot
              (boundary_driver_root worker_rooted dispatch_rooted batch_rooted)
              ShadedDeletion AllocateBlack)
            SatbEdge a \/
          PublishedAlloc a) ->
      (forall a,
          @Collector.CollectorRoot BoundaryAddr StructuralRoot
            (boundary_driver_root worker_rooted dispatch_rooted batch_rooted)
            a ->
          Marked a) ->
      (forall parent child,
          Marked parent -> ReaderEdge parent child -> Marked child) ->
      (forall parent child, InlineEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SideEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SpaceEdge parent child -> ReaderEdge parent child) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a,
          WorkerRoot a ->
          boundary_driver_root worker_rooted dispatch_rooted batch_rooted a) ->
      (forall a,
          SafepointRoot a ->
          boundary_driver_root worker_rooted dispatch_rooted batch_rooted a) ->
      (forall a,
          EnvAnchor a ->
          boundary_driver_root worker_rooted dispatch_rooted batch_rooted a) ->
      (forall a,
          DispatchAnchor a ->
          boundary_driver_root worker_rooted dispatch_rooted batch_rooted a) ->
      (forall a,
          @Collector.Reach BoundaryAddr
            (@Collector.ConcurrentCollectorRoot BoundaryAddr InitialRoot
              (boundary_driver_root worker_rooted dispatch_rooted batch_rooted)
              ShadedDeletion AllocateBlack)
            SatbEdge a ->
          Marked a) ->
      (forall a, PublishedAlloc a -> AllocateBlack a) ->
      (forall a, ReadObserved a -> AddrReturned a) ->
      (forall a, AddrReturned a -> SlotPublished a) ->
      (forall a, SlotPublished a -> SlotWritten a) ->
      (forall a, SlotPublished a -> SegmentPublished a) ->
      (forall a, SegmentPublished a -> SegmentWritten a) ->
      @Collector.ConcurrentFreshOnly BoundaryAddr ConcurrentReturned Fresh ->
      @Collector.FreeListSeparated BoundaryAddr Fresh OnFreeList ->
      (forall a, ReuseReturned a -> OnFreeList a) ->
      (forall a, FutureTouch a -> ~ Freed a) /\
      (forall a,
          ReadObserved a ->
          @Collector.PublishedSlotReady BoundaryAddr SegmentWritten SegmentPublished
            SlotWritten SlotPublished AddrReturned a) /\
      (forall a, ConcurrentReturned a -> ~ ReuseReturned a).
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live
      dispatch_rooted batch_live batch_rooted late_worker_live spawn_latch
      cron_state cron_startup work_pool active_fanout fanout_progress
      classification_lookup e1_default_flip e1_satb_stw_driver
      dedicated_handoff driver_channel StructuralRoot WorkerRoot
      SafepointRoot EnvAnchor DispatchAnchor InitialRoot ShadedDeletion
      AllocateBlack PublishedAlloc SegmentWritten SegmentPublished SlotWritten
      SlotPublished AddrReturned ReadObserved ConcurrentReturned ReuseReturned
      Fresh OnFreeList Marked Freed FutureTouch InlineEdge SideEdge SpaceEdge
      ReaderEdge SatbEdge Hend Hfuture_shape Hcollector_root_marked
      Hreader_closed Hinline Hside Hspace Hsweep Hworker Hsafepoint Henv
      Hdispatch Hsatb_mark Hpublished_black Hread Hreturned Hslot_written
      Hslot_segment Hsegment_written Hfresh Hseparated Hreuse_on_free.
    pose proof
      (end_to_end_safe_implies_gc_window_safe
        w wave
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live
        spawn_latch cron_state cron_startup work_pool active_fanout
        fanout_progress classification_lookup e1_default_flip
        e1_satb_stw_driver dedicated_handoff driver_channel Hend)
      as Hgc_window.
    destruct
      (gc_window_safe_exports_boundary_driver_roots
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live Hgc_window)
      as [Hactive [Hfanout [Hbatch Hadmission_closed]]].
    eapply (@Collector.end_to_end_cesk_index_gc_safety BoundaryAddr);
      eauto.
  Qed.

  Theorem checked_threading_envelope_is_end_to_end_safe :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      end_to_end_safe
        w
        wave
        active_worker_live
        active_worker_live
        true
        true
        true
        true
        false
        complete_spawn_latch
        (cron_final_due
          (cron_worker_complete true
            (cron_second_due (cron_first_due true))))
        complete_startup
        complete_work_pool
        complete_active_fanout
        complete_fanout_progress
        complete_classification_lookup
        complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule.
    unfold end_to_end_safe.
    split.
    - exact Hschedule.
    - split.
      + apply rooted_closed_gc_window_safe.
      + split.
        * apply complete_spawn_latch_safe.
        * split.
          -- apply complete_cron_dispatch_safe.
          -- split.
             ++ apply complete_startup_delivers_submitted_task.
             ++ split.
                ** apply complete_work_pool_envelope_safe.
                ** split.
                   --- unfold active_fanout_envelope_safe.
                       intros _.
                       apply complete_active_fanout_stack_safe.
                   --- split.
                       +++ apply complete_fanout_progress_safe.
                       +++ split.
                           { apply rooted_closed_scheduler_boundary_safe. }
                           { split.
                             - apply complete_classification_lookup_safe.
                             - split.
                               + apply complete_e1_default_flip_safe.
                               + split.
                                 * apply complete_e1_satb_stw_driver_safe.
                                 * split.
                                   -- apply complete_dedicated_handoff_safe.
                                   -- apply complete_driver_channel_protocol_safe. }
  Qed.

  Theorem no_shift_classification_lookup_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          no_shift_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as
      [_ [_ [_ [_ [_ [_ [_ [_ [_ [Hclassification _]]]]]]]]]].
    exact (no_shift_classification_lookup_exposes_overlap Hclassification).
  Qed.

  Theorem legacy_default_ungated_e1_default_flip_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          legacy_default_ungated_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply legacy_default_ungated_exposes_e1_default_flip_gap.
    tauto.
  Qed.

  Theorem trigger_failure_missing_backstop_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          trigger_failure_missing_backstop_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply trigger_failure_missing_backstop_exposes_e1_default_flip_gap.
    tauto.
  Qed.

  Theorem missing_e1_satb_success_release_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          missing_e1_satb_success_release_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply missing_e1_satb_success_release_exposes_driver_gap.
    tauto.
  Qed.

  Theorem missing_e1_satb_abort_stw_backstop_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          missing_e1_satb_abort_stw_backstop_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply missing_e1_satb_abort_stw_backstop_exposes_driver_gap.
    tauto.
  Qed.

  Theorem inline_after_consumed_dedicated_handoff_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          inline_after_consumed_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply inline_after_consumed_exposes_dedicated_handoff_gap.
    tauto.
  Qed.

  Theorem missing_reply_dedicated_handoff_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          missing_reply_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply missing_reply_exposes_dedicated_handoff_gap.
    tauto.
  Qed.

  Theorem missing_request_sender_driver_channel_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          missing_request_sender_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply missing_request_sender_exposes_driver_channel_gap.
    tauto.
  Qed.

  Theorem missing_reply_driver_channel_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          missing_reply_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply missing_reply_exposes_driver_channel_gap.
    tauto.
  Qed.

  Theorem orphan_reply_driver_channel_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          orphan_reply_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply orphan_reply_exposes_driver_channel_gap.
    tauto.
  Qed.

  Theorem fire_and_forget_wait_driver_channel_exposes_end_to_end_gap :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          active_worker_live
          true
          true
          true
          true
          false
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete true
              (cron_second_due (cron_first_due true))))
          complete_startup
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          fire_and_forget_waits_driver_channel.
  Proof.
    intros w wave active_worker_live Hschedule Hend.
    unfold end_to_end_safe in Hend.
    apply fire_and_forget_wait_exposes_driver_channel_gap.
    tauto.
  Qed.

  Theorem missing_cron_startup_poll_path_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          complete_spawn_latch
          cron_state
          missing_poll_path
          complete_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state Hschedule Hgc Hcron
      Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [Hstartup [_ _]]]]]].
    exact (missing_poll_path_exposes_delivery_gap Hstartup).
  Qed.

  Theorem missing_cron_stop_publish_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_startup work_pool active_fanout,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      active_fanout_envelope_safe w active_fanout ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          complete_spawn_latch
          (cron_final_due
            (cron_worker_complete false
              (cron_second_due (cron_first_due true))))
          cron_startup
          work_pool
          active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_startup work_pool
      active_fanout Hschedule Hgc Hstartup Hwork_pool Hactive Hend.
    unfold end_to_end_safe, cron_dispatch_safe,
      cron_stop_prevents_redispatch in Hend.
    simpl in Hend.
    destruct Hend as [_ [_ [_ [[_ Hstop] _]]]].
    discriminate Hstop.
  Qed.

  Theorem spawn_before_latch_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool active_fanout,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      active_fanout_envelope_safe w active_fanout ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          spawn_before_latch
          cron_state
          cron_startup
          work_pool
          active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      active_fanout Hschedule Hgc Hcron Hstartup Hwork_pool Hactive Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [Hlatch _]]].
    exact (spawn_before_latch_exposes_latch_gap Hlatch).
  Qed.

  Theorem missing_dispatch_root_exposes_end_to_end_gap :
    forall w wave cron_state cron_startup work_pool,
      schedule_envelope_safe w wave ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w
          wave
          false
          false
          true
          false
          false
          false
          false
          complete_spawn_latch
          cron_state
          cron_startup
          work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave cron_state cron_startup work_pool Hschedule Hcron Hstartup
      Hwork_pool Hend.
    unfold end_to_end_safe, gc_window_safe, sweep_frees_live_value in Hend.
    simpl in Hend.
    destruct Hend as [_ [Hgc _]].
    discriminate Hgc.
  Qed.

  Theorem missing_batch_root_exposes_end_to_end_gap :
    forall w wave cron_state cron_startup work_pool,
      schedule_envelope_safe w wave ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w
          wave
          false
          false
          false
          false
          true
          false
          false
          complete_spawn_latch
          cron_state
          cron_startup
          work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave cron_state cron_startup work_pool Hschedule Hcron Hstartup
      Hwork_pool Hend.
    unfold end_to_end_safe, gc_window_safe, sweep_frees_live_value in Hend.
    simpl in Hend.
    destruct Hend as [_ [Hgc _]].
    discriminate Hgc.
  Qed.

  Theorem lossy_work_pool_startup_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          complete_spawn_latch
          cron_state
          cron_startup
          lossy_work_pool_startup
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup Hschedule
      Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [_ [Hwork_pool _]]]]]].
    exact (lossy_work_pool_startup_exposes_envelope_gap Hwork_pool).
  Qed.

  Theorem missing_inner_task_panic_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          complete_spawn_latch
          cron_state
          cron_startup
          task_panic_missing_inner_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup Hschedule
      Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [_ [Hwork_pool _]]]]]].
    exact (missing_inner_task_panic_exposes_envelope_gap Hwork_pool).
  Qed.

  Theorem missing_outer_accounting_panic_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          complete_spawn_latch
          cron_state
          cron_startup
          accounting_panic_missing_outer_work_pool
          complete_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup Hschedule
      Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [_ [Hwork_pool _]]]]]].
    exact (missing_outer_accounting_panic_exposes_envelope_gap Hwork_pool).
  Qed.

  Theorem unsafe_work_pool_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool active_fanout,
      ~ work_pool_envelope_safe work_pool ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      active_fanout_envelope_safe w active_fanout ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          complete_spawn_latch
          cron_state
          cron_startup
          work_pool
          active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      active_fanout Hunsafe Hschedule Hgc Hcron Hstartup Hactive Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [_ [Hwork_pool _]]]]]].
    exact (Hunsafe Hwork_pool).
  Qed.

  Theorem uncapped_overflow_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup active_fanout,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      active_fanout_envelope_safe w active_fanout ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup
          uncapped_overflow_work_pool active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup
      active_fanout Hschedule Hgc Hcron Hstartup Hactive.
    eapply unsafe_work_pool_exposes_end_to_end_gap; eauto.
    apply uncapped_overflow_exposes_envelope_gap.
  Qed.

  Theorem double_unpark_overcount_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup active_fanout,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      active_fanout_envelope_safe w active_fanout ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup
          double_unpark_overcounts_work_pool active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup
      active_fanout Hschedule Hgc Hcron Hstartup Hactive.
    eapply unsafe_work_pool_exposes_end_to_end_gap; eauto.
    apply double_unpark_overcount_exposes_envelope_gap.
  Qed.

  Theorem respawn_without_increment_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup active_fanout,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      active_fanout_envelope_safe w active_fanout ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup
          respawn_without_increment_work_pool active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup
      active_fanout Hschedule Hgc Hcron Hstartup Hactive.
    eapply unsafe_work_pool_exposes_end_to_end_gap; eauto.
    apply respawn_without_increment_exposes_envelope_gap.
  Qed.

  Theorem stale_priority_dequeue_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup active_fanout,
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      active_fanout_envelope_safe w active_fanout ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup
          stale_priority_work_pool active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup
      active_fanout Hschedule Hgc Hcron Hstartup Hactive.
    eapply unsafe_work_pool_exposes_end_to_end_gap; eauto.
    apply stale_priority_work_pool_exposes_envelope_gap.
  Qed.

  Theorem unsafe_active_fanout_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool active_fanout,
      uses_direct_fanout w = true ->
      ~ active_fanout_gate_safe active_fanout ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          complete_spawn_latch
          cron_state
          cron_startup
          work_pool
          active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      active_fanout Hdirect Hunsafe Hschedule Hgc Hcron Hstartup Hwork_pool
      Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [_ [_ [Hactive _]]]]]]].
    destruct (Hactive Hdirect) as [Hgate _].
    exact (Hunsafe Hgate).
  Qed.

  Theorem unsafe_active_fanout_stack_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool active_fanout,
      uses_direct_fanout w = true ->
      ~ active_fanout_stack_safe active_fanout ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          dispatch_live
          dispatch_rooted
          batch_live
          batch_rooted
          late_worker_live
          complete_spawn_latch
          cron_state
          cron_startup
          work_pool
          active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      active_fanout Hdirect Hunsafe Hschedule Hgc Hcron Hstartup Hwork_pool
      Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [_ [_ [Hactive _]]]]]]].
    exact (Hunsafe (Hactive Hdirect)).
  Qed.

  Theorem zero_cap_transducer_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          zero_cap_bug_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_stack_exposes_end_to_end_gap; eauto.
    apply zero_cap_bug_active_fanout_exposes_envelope_gap.
  Qed.

  Theorem underutilized_transducer_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          underutilized_transducer_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_stack_exposes_end_to_end_gap; eauto.
    apply underutilized_transducer_exposes_envelope_gap.
  Qed.

  Theorem non_branch_parallel_class_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          non_branch_parallel_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_stack_exposes_end_to_end_gap; eauto.
    apply non_branch_parallel_class_exposes_envelope_gap.
  Qed.

  Theorem missing_dynamic_eval_gate_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          missing_dynamic_eval_gate_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_stack_exposes_end_to_end_gap; eauto.
    apply missing_dynamic_eval_gate_exposes_envelope_gap.
  Qed.

  Theorem state_mutation_bypass_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          state_mutation_bypass_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_stack_exposes_end_to_end_gap; eauto.
    apply state_mutation_bypass_exposes_envelope_gap.
  Qed.

  Theorem strict_io_bypass_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          strict_io_bypass_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_stack_exposes_end_to_end_gap; eauto.
    apply strict_io_bypass_exposes_envelope_gap.
  Qed.

  Theorem missing_purity_active_fanout_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          missing_purity_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_exposes_end_to_end_gap; eauto.
    apply missing_purity_active_fanout_exposes_envelope_gap.
  Qed.

  Theorem missing_budget_active_fanout_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          missing_budget_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_exposes_end_to_end_gap; eauto.
    apply missing_budget_active_fanout_exposes_envelope_gap.
  Qed.

  Theorem partial_dispatch_active_fanout_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted
      dispatch_live dispatch_rooted batch_live batch_rooted late_worker_live
      cron_state cron_startup work_pool,
      uses_direct_fanout w = true ->
      schedule_envelope_safe w wave ->
      gc_window_safe
        active_worker_live worker_rooted
        dispatch_live dispatch_rooted
        batch_live batch_rooted
        late_worker_live ->
      cron_dispatch_safe cron_state ->
      startup_delivery_safe cron_startup ->
      work_pool_envelope_safe work_pool ->
      ~ end_to_end_safe
          w wave active_worker_live worker_rooted
          dispatch_live dispatch_rooted batch_live batch_rooted
          late_worker_live complete_spawn_latch cron_state cron_startup work_pool
          partial_dispatch_active_fanout
          complete_fanout_progress
          complete_classification_lookup
          complete_e1_default_flip
          complete_e1_satb_stw_driver
          complete_dedicated_handoff
          complete_driver_channel.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_exposes_end_to_end_gap; eauto.
    apply partial_dispatch_active_fanout_exposes_envelope_gap.
  Qed.
End EndToEndModel.

End MeTTaTron_GC_ThreadingEndToEndInterleaving.
