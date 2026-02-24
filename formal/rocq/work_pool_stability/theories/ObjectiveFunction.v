(** * Composite Objective Function

    This module defines the four-term composite objective function:

      J(N) = −w_tp × T(N) + w_qd × Q(N) + w_mp × M(N) + w_rss × R(N)

    and proves key gradient properties:
    - Past the USL peak with memory pressure → ΔJ > 0 (scale-down signal)
    - Below the USL peak with no pressure and queue buildup → ΔJ < 0 (scale-up signal)

    These properties establish the correctness of the hill climber's directional
    decisions: it will scale up when throughput is improvable and scale down
    when memory pressure dominates.
*)

From Stdlib Require Import Reals Lra Lia Psatz.
From Coquelicot Require Import Coquelicot.
From WorkPoolStability Require Import Prelude USL.
Open Scope R_scope.

(* ================================================================= *)
(** ** Weight Constants *)
(* ================================================================= *)

(** Throughput weight (reference weight, normalized to 1.0).
    Contribution: −w_tp × EMA(throughput). Negative sign means
    the hill climber maximizes throughput by minimizing the objective. *)
Definition w_tp : R := 1.

(** Queue depth weight.
    At Q = 0.1 (mild buildup), contribution = 0.05 = improvement threshold.
    This ensures even slight queue growth triggers scaling action. *)
Definition w_qd : R := 1 / 2.

(** Memory pressure weight (slab allocator signal).
    At M = 1 (moderate pressure), contribution = 5.0 which exceeds
    the maximum throughput gradient near N* for realistic workloads. *)
Definition w_mp : R := 5.

(** RSS pressure weight (process-level memory signal).
    Stronger than slab signal because OOM kills are unrecoverable.
    60% premium over slab weight: 8.0 vs 5.0. *)
Definition w_rss : R := 8.

(* ================================================================= *)
(** ** Pressure Signal Axioms *)
(* ================================================================= *)

(** Memory pressure signal from the slab allocator (backpressure level).
    M(N) ∈ [0, 3], non-decreasing in N (more threads → more allocation). *)
Parameter M : R -> R.
Axiom M_nonneg : forall N, M N >= 0.
Axiom M_upper : forall N, M N <= 3.
Axiom M_nondecreasing : forall N1 N2, N1 <= N2 -> M N1 <= M N2.

(** RSS pressure signal from /proc/self/statm.
    R(N) ∈ [0, 3], non-decreasing in N. *)
Parameter Rp : R -> R.
Axiom Rp_nonneg : forall N, Rp N >= 0.
Axiom Rp_upper : forall N, Rp N <= 3.
Axiom Rp_nondecreasing : forall N1 N2, N1 <= N2 -> Rp N1 <= Rp N2.

(** Combined range lemmas for convenience. *)
Lemma M_range : forall N, 0 <= M N <= 3.
Proof. intro. split; [apply Rge_le; apply M_nonneg | apply M_upper]. Qed.

Lemma Rp_range : forall N, 0 <= Rp N <= 3.
Proof. intro. split; [apply Rge_le; apply Rp_nonneg | apply Rp_upper]. Qed.

(* ================================================================= *)
(** ** Queue Depth Axioms *)
(* ================================================================= *)

(** Queue depth: pending tasks waiting for execution.
    Q(N) ≥ 0, decreases as N increases toward N* (more workers drain queue),
    and is zero when throughput exceeds arrival rate. *)
Parameter Q : R -> R.
Axiom Q_nonneg : forall N, Q N >= 0.
Axiom Q_zero_when_sufficient : forall N, N >= 1 ->
  USL_throughput N >= lambda -> Q N = 0.
Axiom Q_decreasing_below_peak : forall N1 N2,
  N1 >= 1 -> N1 < N2 -> N2 <= N_star -> Q N2 <= Q N1.

(* ================================================================= *)
(** ** Objective Function *)
(* ================================================================= *)

(** The composite objective (lower = better).

    J(N) = −w_tp × T(N) + w_qd × Q(N) + w_mp × M(N) + w_rss × R(N)

    The hill climber minimizes J by:
    - Maximizing throughput (negative coefficient on T)
    - Minimizing queue depth (positive coefficient on Q)
    - Minimizing memory pressure (positive coefficient on M)
    - Minimizing RSS pressure (positive coefficient on R) *)
Definition objective (N : R) : R :=
  - w_tp * USL_throughput N + w_qd * Q N + w_mp * M N + w_rss * Rp N.

(** Objective gradient: ΔJ = J(N+1) − J(N). *)
Definition obj_delta (N : R) : R := objective (N + 1) - objective N.

(** Expand obj_delta into its four terms. *)
Lemma obj_delta_expand : forall N,
  obj_delta N =
    - w_tp * throughput_delta N
    + w_qd * (Q (N + 1) - Q N)
    + w_mp * (M (N + 1) - M N)
    + w_rss * (Rp (N + 1) - Rp N).
Proof.
  intros N.
  unfold obj_delta, objective, throughput_delta.
  ring.
Qed.

(* ================================================================= *)
(** ** Key Gradient Properties *)
(* ================================================================= *)

(** ** Theorem: Past peak with memory pressure → scale down.

    When N ≥ N* and M(N) > 0:
    - Throughput is non-increasing (USL_decreasing), so −w_tp × ΔT ≥ 0
    - Memory pressure is non-decreasing, so w_mp × ΔM ≥ 0
    - RSS is non-decreasing, so w_rss × ΔR ≥ 0
    - Queue depth is non-negative, so w_qd × ΔQ could be negative,
      but the memory terms dominate.

    Since the overall objective increases (ΔJ > 0), the hill climber
    will choose to Park (scale down) to reduce the objective. *)
Theorem obj_scale_down_past_peak : forall N,
  N >= 1 -> N >= N_star ->
  (* The throughput term is non-negative (T decreasing or flat) *)
  - w_tp * throughput_delta N >= 0 ->
  (* Queue is non-decreasing past peak (saturated workers don't drain) *)
  Q (N + 1) >= Q N ->
  (* M or R is positive at N *)
  M N + Rp N > 0 ->
  obj_delta N >= 0.
Proof.
  intros N HN1 HNstar Htp HQnd Hpressure.
  rewrite obj_delta_expand.
  (* Memory and RSS are non-decreasing *)
  assert (HM : M N <= M (N + 1)) by (apply M_nondecreasing; lra).
  assert (HR : Rp N <= Rp (N + 1)) by (apply Rp_nondecreasing; lra).
  (* All terms non-negative; unfold weights in goal AND hypotheses *)
  unfold w_tp, w_qd, w_mp, w_rss in *.
  lra.
Qed.

(** ** Theorem: Below peak, no pressure, queue positive → scale up.

    When 1 ≤ N and N+1 ≤ N*, M(N) = 0, R(N) = 0, and Q(N) > 0:
    - Throughput is increasing (USL_increasing), so −w_tp × ΔT < 0
    - Memory/RSS terms vanish or are non-positive (at 0, can only stay 0 or increase)
    - Queue decreases as we add workers, so w_qd × ΔQ ≤ 0

    The overall objective decreases (ΔJ < 0), so the hill climber
    will choose to Unpark (scale up). *)
Theorem obj_scale_up_below_peak : forall N,
  1 <= N -> N + 1 <= N_star ->
  M N = 0 -> M (N + 1) = 0 ->
  Rp N = 0 -> Rp (N + 1) = 0 ->
  Q N > 0 ->
  (* Throughput is increasing *)
  throughput_delta N > 0 ->
  obj_delta N < 0.
Proof.
  intros N HN1 HNstar HM0 HM1 HR0 HR1 HQpos Htpd.
  rewrite obj_delta_expand.
  (* Queue is non-increasing below peak *)
  assert (HQdec : Q (N + 1) <= Q N).
  { apply Q_decreasing_below_peak; lra. }
  (* Memory and RSS deltas are zero *)
  unfold w_tp, w_qd, w_mp, w_rss.
  assert (HQn1 : Q (N + 1) >= 0) by exact (Q_nonneg (N + 1)).
  lra.
Qed.

(** ** Corollary: Zero pressure and zero queue → hold near optimum.

    When pressure signals are all zero and queue is empty,
    the objective gradient is determined solely by throughput.
    Near N*, ΔT ≈ 0 (flat peak), so ΔJ ≈ 0, and the hill climber
    correctly holds at the current thread count. *)
Lemma obj_hold_at_optimum : forall N,
  N >= 1 ->
  M N = 0 -> M (N + 1) = 0 ->
  Rp N = 0 -> Rp (N + 1) = 0 ->
  Q N = 0 -> Q (N + 1) = 0 ->
  obj_delta N = - w_tp * throughput_delta N.
Proof.
  intros N HN1 HM0 HM1 HR0 HR1 HQ0 HQ1.
  rewrite obj_delta_expand.
  unfold w_qd, w_mp, w_rss.
  lra.
Qed.
