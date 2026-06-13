(** WorkPool lifecycle accounting obligation.

    Worker parking is a two-level state: each worker owns a parked flag, while
    the pool publishes an aggregate active count. The aggregate may change only
    when the worker flag actually changes. A respawn that deliberately starts a
    previously parked replacement unparked is therefore an unpark transition and
    must increment the aggregate count.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_WorkPoolLifecycle.

Inductive worker_state : Type :=
  | Active
  | Parked.

Definition is_active (s : worker_state) : nat :=
  match s with
  | Active => 1
  | Parked => 0
  end.

Definition is_parked (s : worker_state) : nat :=
  match s with
  | Active => 0
  | Parked => 1
  end.

Definition try_unpark_state (s : worker_state) : worker_state := Active.

Definition try_unpark_count (active : nat) (s : worker_state) : nat :=
  match s with
  | Active => active
  | Parked => S active
  end.

Definition try_park_state (active min_workers : nat) (s : worker_state)
    : worker_state :=
  match s with
  | Active =>
      if active <=? min_workers then Active else Parked
  | Parked => Parked
  end.

Definition try_park_count (active min_workers : nat) (s : worker_state) : nat :=
  match s with
  | Active =>
      if active <=? min_workers then active else active - 1
  | Parked => active
  end.

Definition respawn_unpark_count (active : nat) (previous : worker_state) : nat :=
  try_unpark_count active previous.

Definition consistent (active parked max_workers : nat) : Prop :=
  active + parked = max_workers.

Theorem try_unpark_active_is_noop :
  forall active,
    try_unpark_count active Active = active /\
    try_unpark_state Active = Active.
Proof.
  intros; split; reflexivity.
Qed.

Theorem try_unpark_parked_counts_once :
  forall active,
    try_unpark_count active Parked = S active /\
    try_unpark_state Parked = Active.
Proof.
  intros; split; reflexivity.
Qed.

Theorem double_unpark_no_double_count :
  forall active,
    try_unpark_count
      (try_unpark_count active Parked)
      (try_unpark_state Parked)
    = S active.
Proof.
  intros; reflexivity.
Qed.

Theorem try_park_parked_is_noop :
  forall active min_workers,
    try_park_count active min_workers Parked = active /\
    try_park_state active min_workers Parked = Parked.
Proof.
  intros; split; reflexivity.
Qed.

Theorem try_park_active_above_min_counts_once :
  forall active min_workers,
    min_workers < active ->
    try_park_count active min_workers Active = active - 1 /\
    try_park_state active min_workers Active = Parked.
Proof.
  intros active min_workers Hgt.
  unfold try_park_count, try_park_state.
  assert (Hcmp : (active <=? min_workers) = false) by
    (apply Nat.leb_gt; exact Hgt).
  rewrite Hcmp.
  split; reflexivity.
Qed.

Theorem try_park_active_at_min_is_noop :
  forall active min_workers,
    active <= min_workers ->
    try_park_count active min_workers Active = active /\
    try_park_state active min_workers Active = Active.
Proof.
  intros active min_workers Hle.
  unfold try_park_count, try_park_state.
  assert (Hcmp : (active <=? min_workers) = true) by
    (apply Nat.leb_le; exact Hle).
  rewrite Hcmp.
  split; reflexivity.
Qed.

Theorem double_park_no_double_count :
  forall active min_workers,
    min_workers < active ->
    try_park_count
      (try_park_count active min_workers Active)
      min_workers
      (try_park_state active min_workers Active)
    = active - 1.
Proof.
  intros active min_workers Hgt.
  destruct (try_park_active_above_min_counts_once active min_workers Hgt)
    as [Hcount Hstate].
  rewrite Hcount, Hstate.
  reflexivity.
Qed.

Theorem unpark_preserves_capacity_consistency :
  forall active parked max_workers,
    consistent active (S parked) max_workers ->
    consistent (S active) parked max_workers.
Proof.
  unfold consistent; intros; lia.
Qed.

Theorem park_preserves_capacity_consistency :
  forall active parked max_workers,
    0 < active ->
    consistent active parked max_workers ->
    consistent (active - 1) (S parked) max_workers.
Proof.
  unfold consistent; intros; lia.
Qed.

Theorem respawn_parked_replacement_must_increment :
  forall active,
    respawn_unpark_count active Parked = S active.
Proof.
  intros; reflexivity.
Qed.

Theorem old_respawn_without_increment_breaks_consistency :
  ~ consistent 1 0 2.
Proof.
  unfold consistent.
  lia.
Qed.

End MeTTaTron_GC_WorkPoolLifecycle.
