(** End-to-end threading/scheduler/cron/GC interleaving envelope.

    This proof composes the audit obligations that were previously checked in
    separate files: scheduler reordering, effect-conflict exclusion, direct
    fanout maximality for independent work, active-worker GC rooting, closed
    worker admission during a root snapshot, and recurring cron in_flight
    claims.
*)

From Stdlib Require Import Bool.Bool.
From Stdlib Require Import Arith Lia.
Require Import CronStartupDelivery.
Require Import WorkPoolPanicIsolation.
Require Import WorkPoolStartupDrain.

Import MeTTaTron_GC_CronStartupDelivery.
Import MeTTaTron_GC_WorkPoolPanicIsolation.
Import MeTTaTron_GC_WorkPoolStartupDrain.

Module MeTTaTron_GC_ThreadingEndToEndInterleaving.

Section EndToEndModel.
  Inductive Task : Type :=
  | Producer : Task
  | Consumer : Task.

  Record Workload : Type := {
    has_dependency : bool;
    has_effect_conflict : bool;
    uses_direct_fanout : bool
  }.

  Definition WaveAssignment : Type := Task -> nat.

  Definition dependency_order_sound
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    has_dependency w = true ->
    wave Producer < wave Consumer.

  Definition effect_conflict_sound
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    has_effect_conflict w = true ->
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

  Definition schedule_envelope_safe
      (w : Workload)
      (wave : WaveAssignment)
      : Prop :=
    dependency_order_sound w wave /\
    effect_conflict_sound w wave /\
    independent_parallelism_maximal w wave /\
    direct_fanout_sound w wave.

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
        * exact Hdirect.
  Qed.

  Theorem same_wave_dependency_exposes_reorder :
    forall w wave,
      has_dependency w = true ->
      wave Producer = wave Consumer ->
      ~ dependency_order_sound w wave.
  Proof.
    intros w wave Hdep Hsame Hsound.
    unfold dependency_order_sound in Hsound.
    specialize (Hsound Hdep).
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
    specialize (Hsound Hconflict).
    exact (Hsound Hsame).
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
      (active_worker_live worker_rooted late_worker_live : bool)
      : bool :=
    (active_worker_live && negb worker_rooted) || late_worker_live.

  Definition gc_window_safe
      (active_worker_live worker_rooted late_worker_live : bool)
      : Prop :=
    sweep_frees_live_value active_worker_live worker_rooted late_worker_live =
    false.

  Theorem rooted_closed_gc_window_safe :
    forall active_worker_live,
      gc_window_safe active_worker_live active_worker_live false.
  Proof.
    intros []; reflexivity.
  Qed.

  Theorem missing_active_worker_root_exposes_live_free :
    sweep_frees_live_value true false false = true.
  Proof.
    reflexivity.
  Qed.

  Theorem late_worker_after_snapshot_exposes_live_free :
    forall worker_rooted,
      sweep_frees_live_value false worker_rooted true = true.
  Proof.
    intros []; reflexivity.
  Qed.

  Record CronState : Type := {
    cron_in_flight : bool;
    cron_overlap : bool
  }.

  Definition cron_first_due (claim_before_dispatch : bool) : CronState :=
    {| cron_in_flight := claim_before_dispatch;
       cron_overlap := false |}.

  Definition cron_second_due (s : CronState) : CronState :=
    if cron_in_flight s then
      {| cron_in_flight := true; cron_overlap := cron_overlap s |}
    else
      {| cron_in_flight := false; cron_overlap := true |}.

  Definition cron_no_overlap (s : CronState) : Prop :=
    cron_overlap s = false.

  Theorem claimed_cron_dispatch_prevents_overlap :
    cron_no_overlap (cron_second_due (cron_first_due true)).
  Proof.
    reflexivity.
  Qed.

  Theorem unclaimed_cron_dispatch_exposes_overlap :
    cron_overlap (cron_second_due (cron_first_due false)) = true.
  Proof.
    reflexivity.
  Qed.

  Record WorkPoolConfig : Type := {
    work_pool_submitted : nat;
    work_pool_retained : nat;
    work_pool_inner_catch : bool;
    work_pool_outer_catch : bool;
    work_pool_failure : FailureKind
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

  Definition work_pool_envelope_safe (c : WorkPoolConfig) : Prop :=
    work_pool_startup_safe c /\ work_pool_panic_safe c.

  Definition complete_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := true;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic |}.

  Definition lossy_work_pool_startup : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := 0;
       work_pool_inner_catch := true;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic |}.

  Definition task_panic_missing_inner_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := false;
       work_pool_outer_catch := true;
       work_pool_failure := TaskClosurePanic |}.

  Definition accounting_panic_missing_outer_work_pool : WorkPoolConfig :=
    {| work_pool_submitted := 1;
       work_pool_retained := prestart_queue_after_submissions 1;
       work_pool_inner_catch := true;
       work_pool_outer_catch := false;
       work_pool_failure := AccountingPanic |}.

  Theorem complete_work_pool_envelope_safe :
    work_pool_envelope_safe complete_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_startup_safe,
      work_pool_panic_safe, complete_work_pool,
      prestart_queue_after_submissions.
    simpl.
    split.
    - reflexivity.
    - repeat split.
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
    intros [_ [_ [_ Hheartbeat]]].
    specialize (Hheartbeat eq_refl).
    exact (missing_inner_catch_loses_task_panic_heartbeat Hheartbeat).
  Qed.

  Theorem missing_outer_accounting_panic_exposes_envelope_gap :
    ~ work_pool_envelope_safe accounting_panic_missing_outer_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_panic_safe,
      accounting_panic_missing_outer_work_pool.
    simpl.
    intros [_ [Halive _]].
    exact (missing_outer_catch_can_kill_worker_on_accounting_panic Halive).
  Qed.

  Definition end_to_end_safe
      (w : Workload)
      (wave : WaveAssignment)
      (active_worker_live worker_rooted late_worker_live : bool)
      (cron_state : CronState)
      (cron_startup : StartupConfig)
      (work_pool : WorkPoolConfig)
      : Prop :=
    schedule_envelope_safe w wave /\
    gc_window_safe active_worker_live worker_rooted late_worker_live /\
    cron_no_overlap cron_state /\
    startup_delivery_safe cron_startup /\
    work_pool_envelope_safe work_pool.

  Theorem checked_threading_envelope_is_end_to_end_safe :
    forall w wave active_worker_live,
      schedule_envelope_safe w wave ->
      end_to_end_safe
        w
        wave
        active_worker_live
        active_worker_live
        false
        (cron_second_due (cron_first_due true))
        complete_startup
        complete_work_pool.
  Proof.
    intros w wave active_worker_live Hschedule.
    unfold end_to_end_safe.
    split.
    - exact Hschedule.
    - split.
      + apply rooted_closed_gc_window_safe.
      + split.
        * apply claimed_cron_dispatch_prevents_overlap.
        * split.
          -- apply complete_startup_delivers_submitted_task.
          -- apply complete_work_pool_envelope_safe.
  Qed.

  Theorem missing_cron_startup_poll_path_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted late_worker_live cron_state,
      schedule_envelope_safe w wave ->
      gc_window_safe active_worker_live worker_rooted late_worker_live ->
      cron_no_overlap cron_state ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          late_worker_live
          cron_state
          missing_poll_path
          complete_work_pool.
  Proof.
    intros w wave active_worker_live worker_rooted late_worker_live cron_state
      Hschedule Hgc Hcron Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [Hstartup _]]]].
    exact (missing_poll_path_exposes_delivery_gap Hstartup).
  Qed.

  Theorem lossy_work_pool_startup_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted late_worker_live
      cron_state cron_startup,
      schedule_envelope_safe w wave ->
      gc_window_safe active_worker_live worker_rooted late_worker_live ->
      cron_no_overlap cron_state ->
      startup_delivery_safe cron_startup ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          late_worker_live
          cron_state
          cron_startup
          lossy_work_pool_startup.
  Proof.
    intros w wave active_worker_live worker_rooted late_worker_live cron_state
      cron_startup Hschedule Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ Hwork_pool]]]].
    exact (lossy_work_pool_startup_exposes_envelope_gap Hwork_pool).
  Qed.

  Theorem missing_inner_task_panic_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted late_worker_live
      cron_state cron_startup,
      schedule_envelope_safe w wave ->
      gc_window_safe active_worker_live worker_rooted late_worker_live ->
      cron_no_overlap cron_state ->
      startup_delivery_safe cron_startup ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          late_worker_live
          cron_state
          cron_startup
          task_panic_missing_inner_work_pool.
  Proof.
    intros w wave active_worker_live worker_rooted late_worker_live cron_state
      cron_startup Hschedule Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ Hwork_pool]]]].
    exact (missing_inner_task_panic_exposes_envelope_gap Hwork_pool).
  Qed.

  Theorem missing_outer_accounting_panic_exposes_end_to_end_gap :
    forall w wave active_worker_live worker_rooted late_worker_live
      cron_state cron_startup,
      schedule_envelope_safe w wave ->
      gc_window_safe active_worker_live worker_rooted late_worker_live ->
      cron_no_overlap cron_state ->
      startup_delivery_safe cron_startup ->
      ~ end_to_end_safe
          w
          wave
          active_worker_live
          worker_rooted
          late_worker_live
          cron_state
          cron_startup
          accounting_panic_missing_outer_work_pool.
  Proof.
    intros w wave active_worker_live worker_rooted late_worker_live cron_state
      cron_startup Hschedule Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ Hwork_pool]]]].
    exact (missing_outer_accounting_panic_exposes_envelope_gap Hwork_pool).
  Qed.
End EndToEndModel.

End MeTTaTron_GC_ThreadingEndToEndInterleaving.
