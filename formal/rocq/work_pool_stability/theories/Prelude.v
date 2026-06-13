(** * Prelude: Explicit Model Inputs and Utility Lemmas

    This module defines the system parameters for the WorkPool memory-aware
    scaling model based on the Universal Scalability Law (USL). The range
    evidence is carried by an explicit record, so importing this module adds no
    trusted global declarations.

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
(** ** System Inputs *)
(* ================================================================= *)

(** Physical WorkPool model inputs plus their range evidence. *)
Record WorkPoolParams : Type := {
  (** Single-thread throughput (evals/s). Must be positive. *)
  T1 : R;
  (** Serial fraction: proportion of work that cannot be parallelized. *)
  sigma : R;
  (** Coherence/contention penalty coefficient. *)
  kappa : R;
  (** Task arrival rate (evals/s). *)
  lambda : R;
  T1_pos : T1 > 0;
  sigma_pos : 0 < sigma;
  sigma_lt_1 : sigma < 1;
  kappa_pos : kappa > 0;
  lambda_pos : lambda > 0
}.

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

Lemma sigma_range : forall p : WorkPoolParams, 0 < sigma p < 1.
Proof. intro p. split; [exact (sigma_pos p) | exact (sigma_lt_1 p)]. Qed.

Lemma one_minus_sigma_pos : forall p : WorkPoolParams, 1 - sigma p > 0.
Proof. intro p. pose proof (sigma_lt_1 p). lra. Qed.

Lemma one_minus_sigma_over_kappa_pos :
  forall p : WorkPoolParams, (1 - sigma p) / kappa p > 0.
Proof.
  intro p.
  apply Rdiv_lt_0_compat.
  - pose proof (sigma_lt_1 p). lra.
  - exact (kappa_pos p).
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
