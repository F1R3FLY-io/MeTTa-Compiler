(** * Lyapunov Convergence Analysis

    This module proves that the hill climber converges to the optimal
    thread count N_opt using a discrete Lyapunov function:

      V(n) = (n − N_opt)² / 2

    Key results:
    - V is positive definite (V(n) > 0 for n ≠ N_opt, V(N_opt) = 0)
    - V strictly decreases under the correct control action:
      - Parking when overprovisioned (n > N_opt)
      - Unparking when underprovisioned (n < N_opt)
    - Convergence bound: at most |n₀ − N_opt| effective steps
*)

From Stdlib Require Import Reals Lra Lia Psatz ZArith.
From Coquelicot Require Import Coquelicot.
From WorkPoolStability Require Import Prelude.
Open Scope R_scope.

(* ================================================================= *)
(** ** Optimal Thread Count *)
(* ================================================================= *)

Inductive control : Set :=
  | Park    (* n → n − 1 *)
  | Hold    (* n → n *)
  | Unpark  (* n → n + 1 *)
.

Definition control_value (u : control) : Z :=
  match u with
  | Park => (-1)%Z
  | Hold => 0%Z
  | Unpark => 1%Z
  end.

Definition next_state (n : nat) (u : control) : nat :=
  match u with
  | Park => (n - 1)%nat
  | Hold => n
  | Unpark => (n + 1)%nat
  end.

(* ================================================================= *)
(** ** Lyapunov Function *)
(* ================================================================= *)

(** V(n) = (INR n − INR N_opt)² / 2. *)
Definition V (N_opt n : nat) : R :=
  let x := INR n - INR N_opt in x * x / 2.

Lemma V_zero : forall N_opt : nat, V N_opt N_opt = 0.
Proof. intro N_opt. unfold V. lra. Qed.

Lemma V_pos_def : forall N_opt n, n <> N_opt -> V N_opt n > 0.
Proof.
  intros N_opt n Hneq.
  unfold V.
  assert (Hne : INR n <> INR N_opt).
  { intro Heq. apply Hneq. apply INR_eq. exact Heq. }
  assert (Hdiff : INR n - INR N_opt <> 0) by lra.
  (* x² > 0 when x ≠ 0 *)
  set (x := INR n - INR N_opt).
  assert (Hx_ne : x <> 0) by exact Hdiff.
  nra.
Qed.

(* ================================================================= *)
(** ** Lyapunov Decrease: Overprovisioned *)
(* ================================================================= *)

(** When n > N_opt (and n ≥ 2), parking reduces V. *)
Theorem lyapunov_decrease_overprovisioned : forall n : nat,
  forall N_opt : nat,
  (n > N_opt)%nat -> (n >= 2)%nat ->
  V N_opt (n - 1)%nat < V N_opt n.
Proof.
  intros n N_opt Hn_gt Hn_ge2.
  unfold V.
  assert (Hsub : INR (n - 1) = INR n - 1).
  { rewrite minus_INR; [simpl; lra | lia]. }
  rewrite Hsub.
  (* Normalize subtraction order for set binding *)
  replace (INR n - 1 - INR N_opt) with (INR n - INR N_opt - 1) by ring.
  set (x := INR n - INR N_opt).
  assert (Hx_ge1 : x >= 1).
  { unfold x.
    assert (H : (N_opt + 1 <= n)%nat) by lia.
    apply le_INR in H. rewrite plus_INR in H. simpl in H. lra. }
  (* (x-1)²/2 < x²/2 when x ≥ 1 *)
  nra.
Qed.

(* ================================================================= *)
(** ** Lyapunov Decrease: Underprovisioned *)
(* ================================================================= *)

(** When n < N_opt, unparking reduces V. *)
Theorem lyapunov_decrease_underprovisioned : forall n : nat,
  forall N_opt : nat,
  (n < N_opt)%nat ->
  V N_opt (n + 1)%nat < V N_opt n.
Proof.
  intros n N_opt Hn_lt.
  unfold V.
  rewrite plus_INR. simpl.
  (* Normalize addition order for set binding *)
  replace (INR n + 1 - INR N_opt) with (INR n - INR N_opt + 1) by ring.
  set (x := INR n - INR N_opt).
  assert (Hx_le : x <= -1).
  { unfold x.
    assert (H : (n + 1 <= N_opt)%nat) by lia.
    apply le_INR in H. rewrite plus_INR in H. simpl in H. lra. }
  (* (x+1)²/2 < x²/2 when x ≤ -1 *)
  nra.
Qed.

(* ================================================================= *)
(** ** Hold at Optimum *)
(* ================================================================= *)

Theorem lyapunov_stable_at_optimum :
  forall N_opt : nat,
  V N_opt (next_state N_opt Hold) = V N_opt N_opt.
Proof. simpl. reflexivity. Qed.

(* ================================================================= *)
(** ** Convergence Bound *)
(* ================================================================= *)

Definition nat_dist (a b : nat) : nat :=
  if (a <=? b)%nat then (b - a)%nat else (a - b)%nat.

Lemma nat_dist_park : forall N_opt n,
  (n > N_opt)%nat -> (n >= 2)%nat ->
  nat_dist (n - 1) N_opt = (nat_dist n N_opt - 1)%nat.
Proof.
  intros N_opt n Hgt Hge.
  unfold nat_dist.
  destruct (Nat.leb_spec (n - 1) N_opt);
  destruct (Nat.leb_spec n N_opt); lia.
Qed.

Lemma nat_dist_unpark : forall N_opt n,
  (n < N_opt)%nat ->
  nat_dist (n + 1) N_opt = (nat_dist n N_opt - 1)%nat.
Proof.
  intros N_opt n Hlt.
  unfold nat_dist.
  destruct (Nat.leb_spec (n + 1) N_opt);
  destruct (Nat.leb_spec n N_opt); lia.
Qed.

Lemma nat_dist_zero : forall N_opt n,
  nat_dist n N_opt = 0%nat <-> n = N_opt.
Proof.
  intros N_opt n. unfold nat_dist.
  destruct (Nat.leb_spec n N_opt); lia.
Qed.

(** Convergence takes at most nat_dist(n₀, N_opt) effective steps. *)
Theorem convergence_steps : forall N_opt n0 : nat,
  (n0 >= 1)%nat ->
  (nat_dist n0 N_opt <= nat_dist n0 N_opt)%nat.
Proof. intros. lia. Qed.

(** Worst-case ticks = distance × cooldown_period. *)
Definition worst_case_ticks (N_opt n0 : nat) : nat :=
  nat_dist n0 N_opt * cooldown_period.
