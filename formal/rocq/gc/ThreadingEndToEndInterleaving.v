(** End-to-end threading/scheduler/cron/GC interleaving envelope.

    This proof composes the audit obligations that were previously checked in
    separate files: scheduler reordering, effect-conflict exclusion, direct
    fanout maximality for independent work, active-worker GC rooting, closed
    worker admission during a root snapshot, and recurring cron in_flight
    claims plus stop-before-redispatch.
*)

From Stdlib Require Import Bool.Bool.
From Stdlib Require Import Arith Lia.
Require Import CronRecurringDispatch.
Require Import CronStartupDelivery.
Require Import SchedulerActiveFanoutGate.
Require Import SchedulerDynamicEvalGate.
Require Import SchedulerTransducerParallelism.
Require Import WorkPoolLifecycle.
Require Import WorkPoolOverflowCap.
Require Import WorkPoolPanicIsolation.
Require Import WorkPoolStartupDrain.

Import MeTTaTron_GC_CronRecurringDispatch.
Import MeTTaTron_GC_CronStartupDelivery.
Import MeTTaTron_GC_SchedulerActiveFanoutGate.
Import MeTTaTron_GC_SchedulerDynamicEvalGate.
Import MeTTaTron_GC_SchedulerTransducerParallelism.
Import MeTTaTron_GC_WorkPoolLifecycle.
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
    work_pool_respawn_parked : nat
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

  Definition work_pool_envelope_safe (c : WorkPoolConfig) : Prop :=
    work_pool_startup_safe c /\
    work_pool_panic_safe c /\
    work_pool_overflow_safe c /\
    work_pool_lifecycle_safe c.

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
       work_pool_respawn_parked := 0 |}.

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
       work_pool_respawn_parked := 0 |}.

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
       work_pool_respawn_parked := 0 |}.

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
       work_pool_respawn_parked := 0 |}.

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
       work_pool_respawn_parked := 0 |}.

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
       work_pool_respawn_parked := 0 |}.

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
       work_pool_respawn_parked := 0 |}.

  Theorem complete_work_pool_envelope_safe :
    work_pool_envelope_safe complete_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_startup_safe,
      work_pool_panic_safe, work_pool_overflow_safe,
      work_pool_lifecycle_safe, complete_work_pool,
      prestart_queue_after_submissions.
    simpl.
    unfold consistent.
    repeat split; try reflexivity; lia.
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
    intros [_ [_ [_ Hlife]]].
    destruct Hlife as [_ [_ [_ [_ [Hconsistent _]]]]].
    lia.
  Qed.

  Theorem respawn_without_increment_exposes_envelope_gap :
    ~ work_pool_envelope_safe respawn_without_increment_work_pool.
  Proof.
    unfold work_pool_envelope_safe, work_pool_lifecycle_safe,
      respawn_without_increment_work_pool, consistent.
    simpl.
    intros [_ [_ [_ Hlife]]].
    destruct Hlife as [_ [_ [_ [_ [_ [_ [_ Hconsistent]]]]]]].
    lia.
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

  Definition active_fanout_stack_safe (c : ActiveFanoutConfig) : Prop :=
    active_fanout_gate_safe c /\
    active_transducer_safe c /\
    active_parallel_dispatch_blockers_safe c.

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
      + repeat split.
        * intros Hdyn; discriminate Hdyn.
        * intros Hmutation; discriminate Hmutation.
        * intros Hstrict _; discriminate Hstrict.
        * unfold active_no_budget_parallel_safe, no_budget_parallel_allowed,
            blocks_parallel_dispatch, complete_active_fanout.
          simpl.
          intros _ [Hstate | [[Hstrict _] | [_ Hdyn]]].
          -- discriminate Hstate.
          -- discriminate Hstrict.
          -- discriminate Hdyn.
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
    intros [_ [_ [Hdynamic _]]].
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
    intros [_ [_ [_ [_ [_ Hno_budget]]]]].
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
    intros [_ [_ [_ [_ [_ Hno_budget]]]]].
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

  Definition end_to_end_safe
      (w : Workload)
      (wave : WaveAssignment)
      (active_worker_live worker_rooted
       dispatch_live dispatch_rooted
       batch_live batch_rooted
       late_worker_live : bool)
      (cron_state : CronState)
      (cron_startup : StartupConfig)
      (work_pool : WorkPoolConfig)
      (active_fanout : ActiveFanoutConfig)
      : Prop :=
    schedule_envelope_safe w wave /\
    gc_window_safe
      active_worker_live worker_rooted
      dispatch_live dispatch_rooted
      batch_live batch_rooted
      late_worker_live /\
    cron_dispatch_safe cron_state /\
    startup_delivery_safe cron_startup /\
    work_pool_envelope_safe work_pool /\
    active_fanout_envelope_safe w active_fanout.

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
        (cron_final_due
          (cron_worker_complete true
            (cron_second_due (cron_first_due true))))
        complete_startup
        complete_work_pool
        complete_active_fanout.
  Proof.
    intros w wave active_worker_live Hschedule.
    unfold end_to_end_safe.
    split.
    - exact Hschedule.
    - split.
      + apply rooted_closed_gc_window_safe.
      + split.
        * apply complete_cron_dispatch_safe.
        * split.
          -- apply complete_startup_delivers_submitted_task.
          -- split.
             ++ apply complete_work_pool_envelope_safe.
             ++ unfold active_fanout_envelope_safe.
                intros _.
                apply complete_active_fanout_stack_safe.
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
          cron_state
          missing_poll_path
          complete_work_pool
          complete_active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state Hschedule Hgc Hcron
      Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [Hstartup [_ _]]]]].
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
          (cron_final_due
            (cron_worker_complete false
              (cron_second_due (cron_first_due true))))
          cron_startup
          work_pool
          active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_startup work_pool
      active_fanout Hschedule Hgc Hstartup Hwork_pool Hactive Hend.
    unfold end_to_end_safe, cron_dispatch_safe,
      cron_stop_prevents_redispatch in Hend.
    simpl in Hend.
    destruct Hend as [_ [_ [[_ Hstop] _]]].
    discriminate Hstop.
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
          cron_state
          cron_startup
          work_pool
          complete_active_fanout.
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
          cron_state
          cron_startup
          work_pool
          complete_active_fanout.
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
          cron_state
          cron_startup
          lossy_work_pool_startup
          complete_active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup Hschedule
      Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [Hwork_pool _]]]]].
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
          cron_state
          cron_startup
          task_panic_missing_inner_work_pool
          complete_active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup Hschedule
      Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [Hwork_pool _]]]]].
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
          cron_state
          cron_startup
          accounting_panic_missing_outer_work_pool
          complete_active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup Hschedule
      Hgc Hcron Hstartup Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [Hwork_pool _]]]]].
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
          cron_state
          cron_startup
          work_pool
          active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      active_fanout Hunsafe Hschedule Hgc Hcron Hstartup Hactive Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [Hwork_pool _]]]]].
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
          late_worker_live cron_state cron_startup
          uncapped_overflow_work_pool active_fanout.
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
          late_worker_live cron_state cron_startup
          double_unpark_overcounts_work_pool active_fanout.
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
          late_worker_live cron_state cron_startup
          respawn_without_increment_work_pool active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup
      active_fanout Hschedule Hgc Hcron Hstartup Hactive.
    eapply unsafe_work_pool_exposes_end_to_end_gap; eauto.
    apply respawn_without_increment_exposes_envelope_gap.
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
          cron_state
          cron_startup
          work_pool
          active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      active_fanout Hdirect Hunsafe Hschedule Hgc Hcron Hstartup Hwork_pool
      Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [_ Hactive]]]]].
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
          cron_state
          cron_startup
          work_pool
          active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      active_fanout Hdirect Hunsafe Hschedule Hgc Hcron Hstartup Hwork_pool
      Hend.
    unfold end_to_end_safe in Hend.
    destruct Hend as [_ [_ [_ [_ [_ Hactive]]]]].
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
          late_worker_live cron_state cron_startup work_pool
          zero_cap_bug_active_fanout.
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
          late_worker_live cron_state cron_startup work_pool
          underutilized_transducer_active_fanout.
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
          late_worker_live cron_state cron_startup work_pool
          non_branch_parallel_active_fanout.
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
          late_worker_live cron_state cron_startup work_pool
          missing_dynamic_eval_gate_active_fanout.
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
          late_worker_live cron_state cron_startup work_pool
          state_mutation_bypass_active_fanout.
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
          late_worker_live cron_state cron_startup work_pool
          strict_io_bypass_active_fanout.
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
          late_worker_live cron_state cron_startup work_pool
          missing_purity_active_fanout.
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
          late_worker_live cron_state cron_startup work_pool
          missing_budget_active_fanout.
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
          late_worker_live cron_state cron_startup work_pool
          partial_dispatch_active_fanout.
  Proof.
    intros w wave active_worker_live worker_rooted dispatch_live dispatch_rooted
      batch_live batch_rooted late_worker_live cron_state cron_startup work_pool
      Hdirect Hschedule Hgc Hcron Hstartup Hwork_pool.
    eapply unsafe_active_fanout_exposes_end_to_end_gap; eauto.
    apply partial_dispatch_active_fanout_exposes_envelope_gap.
  Qed.
End EndToEndModel.

End MeTTaTron_GC_ThreadingEndToEndInterleaving.
