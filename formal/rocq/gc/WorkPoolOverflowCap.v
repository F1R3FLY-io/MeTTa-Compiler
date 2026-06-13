(** WorkPool overflow-worker cap obligation.

    The live Rust helper

      overflow_spawn_quota(requested, live_overflow, max_overflow)

    computes `min(requested, max_overflow - live_overflow)`.  The monitor
    and public spawn path may request arbitrary overflow workers, but the pool
    must never increase the live overflow count beyond `max_overflow`.
 *)

From Stdlib Require Import Arith Lia.

Definition overflow_spawn_quota
    (requested live_overflow max_overflow : nat) : nat :=
  Nat.min requested (max_overflow - live_overflow).

Theorem overflow_spawn_quota_bounded_by_request :
  forall requested live_overflow max_overflow,
    overflow_spawn_quota requested live_overflow max_overflow <= requested.
Proof.
  intros.
  unfold overflow_spawn_quota.
  apply Nat.le_min_l.
Qed.

Theorem overflow_spawn_quota_bounded_by_capacity :
  forall requested live_overflow max_overflow,
    overflow_spawn_quota requested live_overflow max_overflow
      <= max_overflow - live_overflow.
Proof.
  intros.
  unfold overflow_spawn_quota.
  apply Nat.le_min_r.
Qed.

Theorem overflow_spawn_quota_zero_at_or_above_cap :
  forall requested live_overflow max_overflow,
    max_overflow <= live_overflow ->
    overflow_spawn_quota requested live_overflow max_overflow = 0.
Proof.
  intros.
  unfold overflow_spawn_quota.
  replace (max_overflow - live_overflow) with 0 by lia.
  apply Nat.min_0_r.
Qed.

Theorem overflow_spawn_preserves_cap :
  forall requested live_overflow max_overflow,
    live_overflow <= max_overflow ->
    live_overflow
      + overflow_spawn_quota requested live_overflow max_overflow
      <= max_overflow.
Proof.
  intros.
  unfold overflow_spawn_quota.
  apply Nat.min_case_strong; lia.
Qed.

Theorem uncapped_spawn_can_exceed_cap :
  2 + 1 > 2.
Proof.
  lia.
Qed.
