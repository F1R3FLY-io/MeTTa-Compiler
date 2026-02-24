(** * Universal Scalability Law (USL) Model

    This module formalizes the USL throughput model:

      T(N) = T1 * N / (1 + sigma*(N-1) + kappa*N*(N-1))

    and proves key properties:
    - The denominator is positive for all N >= 1
    - The derivative numerator simplifies to 1 - sigma - kappa*N*N
    - The peak thread count N_star = sqrt((1-sigma)/kappa) is the unique positive root
    - T(N) is increasing for N < N_star and decreasing for N > N_star
*)

From Stdlib Require Import Reals Lra Lia Psatz.
From Coquelicot Require Import Coquelicot.
From WorkPoolStability Require Import Prelude.
Open Scope R_scope.

(* ================================================================= *)
(** ** USL Denominator *)
(* ================================================================= *)

Definition USL_denom (N : R) : R :=
  1 + sigma * (N - 1) + kappa * N * (N - 1).

Lemma USL_denom_pos : forall N, N >= 1 -> USL_denom N > 0.
Proof.
  intros N HN.
  unfold USL_denom.
  pose proof sigma_pos.
  pose proof kappa_pos.
  assert (HN1 : N - 1 >= 0) by lra.
  assert (Hs : 0 <= sigma * (N - 1)).
  { apply Rmult_le_pos; lra. }
  assert (Hk : 0 <= kappa * N * (N - 1)).
  { apply Rmult_le_pos.
    - apply Rmult_le_pos; lra.
    - lra. }
  lra.
Qed.

(* ================================================================= *)
(** ** USL Throughput *)
(* ================================================================= *)

Definition USL_throughput (N : R) : R :=
  T1 * N / USL_denom N.

Lemma USL_at_one : USL_throughput 1 = T1.
Proof.
  unfold USL_throughput, USL_denom.
  field.
Qed.

(* ================================================================= *)
(** ** Derivative Analysis *)
(* ================================================================= *)

Lemma USL_deriv_numerator : forall N,
  USL_denom N - N * (sigma + kappa * (2 * N - 1)) = 1 - sigma - kappa * N * N.
Proof.
  intros N. unfold USL_denom. ring.
Qed.

Definition USL_sign (N : R) : R := 1 - sigma - kappa * N * N.

(* ================================================================= *)
(** ** Peak Thread Count *)
(* ================================================================= *)

Definition N_star : R := sqrt ((1 - sigma) / kappa).

Lemma N_star_pos : N_star > 0.
Proof.
  unfold N_star.
  apply sqrt_lt_R0.
  exact one_minus_sigma_over_kappa_pos.
Qed.

Lemma N_star_sq : N_star * N_star = (1 - sigma) / kappa.
Proof.
  unfold N_star.
  rewrite sqrt_sqrt.
  - reflexivity.
  - left. exact one_minus_sigma_over_kappa_pos.
Qed.

(** Key identity: kappa * N_star * N_star = 1 - sigma. *)
Lemma kappa_N_star_sq : kappa * N_star * N_star = 1 - sigma.
Proof.
  pose proof kappa_pos.
  pose proof N_star_sq.
  (* kappa * N_star * N_star = kappa * (N_star * N_star) = kappa * ((1-sigma)/kappa) = 1-sigma *)
  replace (kappa * N_star * N_star) with (kappa * (N_star * N_star)) by ring.
  rewrite N_star_sq.
  field. lra.
Qed.

Theorem USL_peak : USL_sign N_star = 0.
Proof.
  unfold USL_sign.
  pose proof kappa_N_star_sq. lra.
Qed.

(* ================================================================= *)
(** ** Monotonicity *)
(* ================================================================= *)

(** Helper: for positive reals, a < b implies a*a < b*b. *)
Lemma sq_strict_mono : forall a b, 0 <= a -> a < b -> a * a < b * b.
Proof.
  intros a b Ha Hab.
  assert (b - a > 0) by lra.
  assert (b + a >= 0) by lra.
  assert (b + a > 0) by lra.
  assert ((b - a) * (b + a) > 0).
  { apply Rmult_lt_0_compat; lra. }
  lra.
Qed.

(** T is increasing for 1 <= N < N_star. *)
Theorem USL_increasing : forall N, 1 <= N -> N < N_star ->
  USL_sign N > 0.
Proof.
  intros N HN1 HNstar.
  unfold USL_sign.
  pose proof kappa_pos.
  pose proof kappa_N_star_sq.
  assert (HNsq : N * N < N_star * N_star).
  { apply sq_strict_mono; lra. }
  (* kappa * N * N < kappa * N_star * N_star = 1 - sigma *)
  assert (Hk : kappa * N * N < 1 - sigma).
  { assert (0 <= kappa * N * N).
    { apply Rmult_le_pos.
      - apply Rmult_le_pos; lra.
      - lra. }
    assert (kappa * (N * N) < kappa * (N_star * N_star)).
    { apply Rmult_lt_compat_l; lra. }
    lra. }
  lra.
Qed.

(** T is decreasing for N > N_star. *)
Theorem USL_decreasing : forall N, N > N_star ->
  USL_sign N < 0.
Proof.
  intros N HN.
  unfold USL_sign.
  pose proof kappa_pos.
  pose proof N_star_pos.
  pose proof kappa_N_star_sq.
  assert (HNsq : N_star * N_star < N * N).
  { apply sq_strict_mono; lra. }
  assert (Hk : 1 - sigma < kappa * N * N).
  { assert (kappa * (N_star * N_star) < kappa * (N * N)).
    { apply Rmult_lt_compat_l; lra. }
    lra. }
  lra.
Qed.

(* ================================================================= *)
(** ** Uniqueness of Peak *)
(* ================================================================= *)

Theorem USL_peak_unique : forall N1 N2,
  N1 > 0 -> N2 > 0 ->
  USL_sign N1 = 0 -> USL_sign N2 = 0 ->
  N1 = N2.
Proof.
  intros N1 N2 H1pos H2pos H1zero H2zero.
  unfold USL_sign in *.
  pose proof kappa_pos as Hkp.
  assert (Hsq : N1 * N1 = N2 * N2).
  { apply Rmult_eq_reg_l with (r := kappa); lra. }
  assert (Hprod : (N1 - N2) * (N1 + N2) = 0) by lra.
  assert (Hsum : N1 + N2 > 0) by lra.
  assert (Hdiff : N1 - N2 = 0).
  { destruct (Rmult_integral _ _ Hprod) as [Hd | Hs]; lra. }
  lra.
Qed.

(* ================================================================= *)
(** ** Discrete Throughput Delta *)
(* ================================================================= *)

Definition throughput_delta (N : R) : R :=
  USL_throughput (N + 1) - USL_throughput N.
