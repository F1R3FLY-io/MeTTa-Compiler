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

Definition USL_denom (p : WorkPoolParams) (N : R) : R :=
  1 + sigma p * (N - 1) + kappa p * N * (N - 1).

Lemma USL_denom_pos : forall (p : WorkPoolParams) N,
  N >= 1 -> USL_denom p N > 0.
Proof.
  intros p N HN.
  unfold USL_denom.
  pose proof (sigma_pos p).
  pose proof (kappa_pos p).
  assert (HN1 : N - 1 >= 0) by lra.
  assert (Hs : 0 <= sigma p * (N - 1)).
  { apply Rmult_le_pos; lra. }
  assert (Hk : 0 <= kappa p * N * (N - 1)).
  { apply Rmult_le_pos.
    - apply Rmult_le_pos; lra.
    - lra. }
  lra.
Qed.

(* ================================================================= *)
(** ** USL Throughput *)
(* ================================================================= *)

Definition USL_throughput (p : WorkPoolParams) (N : R) : R :=
  T1 p * N / USL_denom p N.

Lemma USL_at_one : forall p : WorkPoolParams, USL_throughput p 1 = T1 p.
Proof.
  intro p.
  unfold USL_throughput, USL_denom.
  field.
Qed.

(* ================================================================= *)
(** ** Derivative Analysis *)
(* ================================================================= *)

Lemma USL_deriv_numerator : forall N,
  forall p : WorkPoolParams,
  USL_denom p N - N * (sigma p + kappa p * (2 * N - 1)) =
    1 - sigma p - kappa p * N * N.
Proof.
  intros N p. unfold USL_denom. ring.
Qed.

Definition USL_sign (p : WorkPoolParams) (N : R) : R :=
  1 - sigma p - kappa p * N * N.

(* ================================================================= *)
(** ** Peak Thread Count *)
(* ================================================================= *)

Definition N_star (p : WorkPoolParams) : R :=
  sqrt ((1 - sigma p) / kappa p).

Lemma N_star_pos : forall p : WorkPoolParams, N_star p > 0.
Proof.
  intro p.
  unfold N_star.
  apply sqrt_lt_R0.
  exact (one_minus_sigma_over_kappa_pos p).
Qed.

Lemma N_star_sq : forall p : WorkPoolParams,
  N_star p * N_star p = (1 - sigma p) / kappa p.
Proof.
  intro p.
  unfold N_star.
  rewrite sqrt_sqrt.
  - reflexivity.
  - left. exact (one_minus_sigma_over_kappa_pos p).
Qed.

(** Key identity: kappa * N_star * N_star = 1 - sigma. *)
Lemma kappa_N_star_sq : forall p : WorkPoolParams,
  kappa p * N_star p * N_star p = 1 - sigma p.
Proof.
  intro p.
  pose proof (kappa_pos p).
  pose proof (N_star_sq p).
  (* kappa * N_star * N_star = kappa * (N_star * N_star) = kappa * ((1-sigma)/kappa) = 1-sigma *)
  replace (kappa p * N_star p * N_star p)
    with (kappa p * (N_star p * N_star p)) by ring.
  rewrite N_star_sq.
  field. lra.
Qed.

Theorem USL_peak : forall p : WorkPoolParams, USL_sign p (N_star p) = 0.
Proof.
  intro p.
  unfold USL_sign.
  pose proof (kappa_N_star_sq p). lra.
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
Theorem USL_increasing : forall (p : WorkPoolParams) N,
  1 <= N -> N < N_star p ->
  USL_sign p N > 0.
Proof.
  intros p N HN1 HNstar.
  unfold USL_sign.
  pose proof (kappa_pos p).
  pose proof (kappa_N_star_sq p).
  assert (HNsq : N * N < N_star p * N_star p).
  { apply sq_strict_mono; lra. }
  (* kappa * N * N < kappa * N_star * N_star = 1 - sigma *)
  assert (Hk : kappa p * N * N < 1 - sigma p).
  { assert (0 <= kappa p * N * N).
    { apply Rmult_le_pos.
      - apply Rmult_le_pos; lra.
      - lra. }
    assert (kappa p * (N * N) < kappa p * (N_star p * N_star p)).
    { apply Rmult_lt_compat_l; lra. }
    lra. }
  lra.
Qed.

(** T is decreasing for N > N_star. *)
Theorem USL_decreasing : forall (p : WorkPoolParams) N,
  N > N_star p -> USL_sign p N < 0.
Proof.
  intros p N HN.
  unfold USL_sign.
  pose proof (kappa_pos p).
  pose proof (N_star_pos p).
  pose proof (kappa_N_star_sq p).
  assert (HNsq : N_star p * N_star p < N * N).
  { apply sq_strict_mono; lra. }
  assert (Hk : 1 - sigma p < kappa p * N * N).
  { assert (kappa p * (N_star p * N_star p) < kappa p * (N * N)).
    { apply Rmult_lt_compat_l; lra. }
    lra. }
  lra.
Qed.

(* ================================================================= *)
(** ** Uniqueness of Peak *)
(* ================================================================= *)

Theorem USL_peak_unique : forall (p : WorkPoolParams) N1 N2,
  N1 > 0 -> N2 > 0 ->
  USL_sign p N1 = 0 -> USL_sign p N2 = 0 ->
  N1 = N2.
Proof.
  intros p N1 N2 H1pos H2pos H1zero H2zero.
  unfold USL_sign in *.
  pose proof (kappa_pos p) as Hkp.
  assert (Hsq : N1 * N1 = N2 * N2).
  { apply Rmult_eq_reg_l with (r := kappa p); lra. }
  assert (Hprod : (N1 - N2) * (N1 + N2) = 0) by lra.
  assert (Hsum : N1 + N2 > 0) by lra.
  assert (Hdiff : N1 - N2 = 0).
  { destruct (Rmult_integral _ _ Hprod) as [Hd | Hs]; lra. }
  lra.
Qed.

(* ================================================================= *)
(** ** Discrete Throughput Delta *)
(* ================================================================= *)

Definition throughput_delta (p : WorkPoolParams) (N : R) : R :=
  USL_throughput p (N + 1) - USL_throughput p N.
