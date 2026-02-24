(** * Prelude: Shared Axioms, Parameter Ranges, and Utility Lemmas

    This module defines the system parameters for the WorkPool memory-aware
    scaling model based on the Universal Scalability Law (USL). All parameters
    are axiomatized as positive reals with physically justified range constraints.

    The model captures:
    - T₁: single-thread throughput (evals/s)
    - σ: serial fraction (Amdahl's law component)
    - κ: coherence/contention penalty (USL extension)
    - λ: task arrival rate (evals/s)
*)

From Stdlib Require Import Reals Lra Lia Psatz.
From Coquelicot Require Import Coquelicot.
Open Scope R_scope.

(* ================================================================= *)
(** ** System Parameters *)
(* ================================================================= *)

(** Single-thread throughput (evals/s). Must be positive. *)
Parameter T1 : R.
Axiom T1_pos : T1 > 0.

(** Serial fraction: proportion of work that cannot be parallelized.
    Strictly between 0 and 1 (pure serial or pure parallel are degenerate). *)
Parameter sigma : R.
Axiom sigma_pos : 0 < sigma.
Axiom sigma_lt_1 : sigma < 1.

(** Coherence/contention penalty coefficient. Strictly positive.
    In the USL model, κ captures the cost of maintaining cache coherence
    across N processors, which grows as O(N²). *)
Parameter kappa : R.
Axiom kappa_pos : kappa > 0.

(** Task arrival rate (evals/s). Strictly positive. *)
Parameter lambda : R.
Axiom lambda_pos : lambda > 0.

(* ================================================================= *)
(** ** Derived Constants *)
(* ================================================================= *)

(** Embed natural number as real. *)
Definition N_real (n : nat) : R := INR n.

(** Improvement threshold for the hill climber dead zone. *)
Definition threshold : R := 5 / 100.

(** Cooldown period in ticks (5 ticks × 200ms = 1s settling time). *)
Definition cooldown_period : nat := 5.

(** EMA smoothing factor (α = 0.15).
    Half-life ≈ ln(2) / ln(1/(1−α)) ≈ 4.3 samples ≈ 860ms. *)
Definition ema_alpha : R := 15 / 100.

(** EMA gain factor: α / (1 − α).
    This is the steady-state ratio of a step-response to the step magnitude
    after one EMA half-life. Used in weight dominance conditions. *)
Definition ema_gain : R := ema_alpha / (1 - ema_alpha).

(* ================================================================= *)
(** ** Utility Lemmas *)
(* ================================================================= *)

Lemma sigma_range : 0 < sigma < 1.
Proof. split; [exact sigma_pos | exact sigma_lt_1]. Qed.

Lemma one_minus_sigma_pos : 1 - sigma > 0.
Proof. pose proof sigma_lt_1. lra. Qed.

Lemma one_minus_sigma_over_kappa_pos : (1 - sigma) / kappa > 0.
Proof.
  apply Rdiv_lt_0_compat.
  - pose proof sigma_lt_1. lra.
  - exact kappa_pos.
Qed.

Lemma ema_alpha_pos : ema_alpha > 0.
Proof. unfold ema_alpha. lra. Qed.

Lemma ema_alpha_lt_1 : ema_alpha < 1.
Proof. unfold ema_alpha. lra. Qed.

Lemma one_minus_ema_alpha_pos : 1 - ema_alpha > 0.
Proof. unfold ema_alpha. lra. Qed.

Lemma ema_gain_pos : ema_gain > 0.
Proof.
  unfold ema_gain, ema_alpha.
  apply Rdiv_lt_0_compat.
  - lra.
  - lra.
Qed.

Lemma ema_gain_value : ema_gain = 15 / 85.
Proof.
  unfold ema_gain, ema_alpha.
  field_simplify. lra.
Qed.

Lemma threshold_pos : threshold > 0.
Proof. unfold threshold. lra. Qed.

(** N ≥ 1 as a real number implies the natural encoding is at least 1. *)
Lemma INR_ge_1 : forall n : nat, (n >= 1)%nat -> INR n >= 1.
Proof.
  intros n Hn.
  replace 1%nat with (S 0) in Hn by lia.
  apply le_INR in Hn.
  simpl in Hn. lra.
Qed.
