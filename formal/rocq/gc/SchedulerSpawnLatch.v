(** Scheduler eval-worker spawn latch obligation.

    The single-threaded mid-loop index-GC gate is sound only while no eval
    worker has ever been introduced into the process.  The Rust boundary uses
    a sticky latch, [note_worker_spawned()], and each eval-worker handoff must
    store that latch before submitting the closure to the pool.  This proof
    isolates the ordering obligation: once a worker can exist at a later
    collection check, latch-before-spawn makes the latch visible at that check,
    so the mid-loop gate's [~worker_ever_spawned()] conjunct closes the gate.

    The negative theorem is the proof-shaped counterexample: if the spawn is
    allowed before the latch store, a worker can exist while the mid-loop gate
    still observes the latch as false.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_SchedulerSpawnLatch.

Record SpawnTrace := {
  latch_at : nat;
  spawn_at : nat;
  check_at : nat
}.

Definition latch_before_spawn (tr : SpawnTrace) : Prop :=
  latch_at tr <= spawn_at tr.

Definition worker_exists_at_check (tr : SpawnTrace) : Prop :=
  spawn_at tr <= check_at tr.

Definition latch_visible_at_check (tr : SpawnTrace) : Prop :=
  latch_at tr <= check_at tr.

Definition midloop_gate_open_at_check
    (index_mode fanout_disabled : Prop)
    (active_evaluators : nat)
    (tr : SpawnTrace) : Prop :=
  index_mode /\
  fanout_disabled /\
  active_evaluators = 1 /\
  ~ latch_visible_at_check tr.

Theorem latch_before_spawn_visible_when_worker_exists :
  forall tr,
    latch_before_spawn tr ->
    worker_exists_at_check tr ->
    latch_visible_at_check tr.
Proof.
  intros tr Hbefore Hworker.
  unfold latch_before_spawn, worker_exists_at_check,
         latch_visible_at_check in *.
  lia.
Qed.

Theorem visible_latch_closes_midloop_gate :
  forall tr index_mode fanout_disabled active_evaluators,
    latch_visible_at_check tr ->
    ~ midloop_gate_open_at_check
        index_mode fanout_disabled active_evaluators tr.
Proof.
  intros tr index_mode fanout_disabled active_evaluators Hvisible Hgate.
  unfold midloop_gate_open_at_check in Hgate.
  destruct Hgate as [_ [_ [_ Hnot_visible]]].
  apply Hnot_visible.
  exact Hvisible.
Qed.

Theorem latch_before_spawn_blocks_worker_midloop_overlap :
  forall tr index_mode fanout_disabled active_evaluators,
    latch_before_spawn tr ->
    worker_exists_at_check tr ->
    ~ midloop_gate_open_at_check
        index_mode fanout_disabled active_evaluators tr.
Proof.
  intros tr index_mode fanout_disabled active_evaluators Hbefore Hworker.
  apply visible_latch_closes_midloop_gate.
  apply latch_before_spawn_visible_when_worker_exists.
  - exact Hbefore.
  - exact Hworker.
Qed.

Theorem concrete_spawn_before_latch_exposes_unlatched_worker :
  worker_exists_at_check {| latch_at := 2; spawn_at := 0; check_at := 1 |} /\
  midloop_gate_open_at_check True True 1
    {| latch_at := 2; spawn_at := 0; check_at := 1 |}.
Proof.
  split.
  - unfold worker_exists_at_check. simpl. lia.
  - unfold midloop_gate_open_at_check, latch_visible_at_check.
    simpl.
    repeat split.
    lia.
Qed.

Theorem spawn_before_latch_exposes_unlatched_worker :
  exists tr,
    worker_exists_at_check tr /\
    midloop_gate_open_at_check True True 1 tr.
Proof.
  exists {| latch_at := 2; spawn_at := 0; check_at := 1 |}.
  apply concrete_spawn_before_latch_exposes_unlatched_worker.
Qed.

End MeTTaTron_GC_SchedulerSpawnLatch.
