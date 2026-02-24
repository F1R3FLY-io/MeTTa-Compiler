(** * Weight Dominance Verification

    This module verifies that the concrete weight values
    (w_tp = 1.0, w_qd = 0.5, w_mp = 5.0, w_rss = 8.0)
    satisfy the dominance conditions required for correct scaling behavior.

    Each condition ensures that the right signal dominates in the right regime:
    - Memory pressure overrides throughput near the USL peak
    - Scale-up from low N dominates mild memory pressure
    - RSS dominates slab pressure (OOM kills are unrecoverable)
    - Queue depth sensitivity matches the improvement threshold
*)

From Stdlib Require Import Reals Lra Psatz.
From WorkPoolStability Require Import Prelude ObjectiveFunction.
Open Scope R_scope.

(* ================================================================= *)
(** ** Condition 1: Memory Overrides Throughput at Peak *)
(* ================================================================= *)

(** At N = N*, dT/dN = 0, so any positive memory pressure produces
    a positive ΔJ. The memory weight need only be positive for this,
    but we verify the specific value is sufficient. *)
Theorem weight_dominance_at_peak :
  w_mp * 1 > 0.
Proof.
  unfold w_mp. lra.
Qed.

(** At the peak, even minimal pressure (M = ε > 0) produces a positive
    memory contribution that dominates the zero throughput gradient. *)
Corollary memory_dominates_zero_gradient : forall eps,
  eps > 0 -> w_mp * eps > 0.
Proof.
  intros eps Heps. unfold w_mp. lra.
Qed.

(* ================================================================= *)
(** ** Condition 2: Memory Overrides Throughput Near Peak (Parametric) *)
(* ================================================================= *)

(** Near N*, the throughput gradient ΔT is bounded by some delta_T_max.
    After EMA smoothing, the contribution to the objective is:
      w_tp × delta_T_max × ema_gain

    For the memory signal to override, we need:
      w_mp × 1 > w_tp × delta_T_max × ema_gain

    This holds for delta_T_max ≤ 28 evals/s (well above any realistic
    workload's gradient near peak, which is typically 5–9 evals/s). *)
Theorem weight_dominance_near_peak : forall delta_T_max,
  0 <= delta_T_max ->
  delta_T_max <= 28 ->
  w_mp * 1 > w_tp * delta_T_max * ema_gain.
Proof.
  intros delta_T_max Hnneg Hbound.
  unfold w_mp, w_tp, ema_gain, ema_alpha.
  assert (H85 : 1 - 15 / 100 > 0) by lra.
  apply Rmult_lt_reg_r with (r := (1 - 15 / 100)).
  - lra.
  - field_simplify; lra.
Qed.

(** Concrete instantiation: for the typical near-peak gradient of ≤ 9 evals/s. *)
Corollary weight_dominance_typical :
  w_mp * 1 > w_tp * 9 * ema_gain.
Proof.
  apply weight_dominance_near_peak; lra.
Qed.

(* ================================================================= *)
(** ** Condition 3: RSS Dominates Slab Pressure *)
(* ================================================================= *)

(** The RSS weight is 60% stronger than the slab weight.
    This prioritizes process-level memory (OOM kill risk) over
    slab-level pressure (which can be mitigated by GC). *)
Theorem rss_dominates_slab : w_rss > w_mp.
Proof.
  unfold w_rss, w_mp. lra.
Qed.

(** Quantified: RSS is 1.6× the slab weight. *)
Lemma rss_slab_ratio : w_rss / w_mp = 8 / 5.
Proof.
  unfold w_rss, w_mp. field.
Qed.

(* ================================================================= *)
(** ** Condition 4: Queue Sensitivity Matches Threshold *)
(* ================================================================= *)

(** At mild queue buildup (Q = 0.1), the queue contribution exactly
    equals the improvement threshold. This means even a small queue
    buildup is sufficient to trigger scaling action. *)
Theorem queue_threshold_sensitivity : w_qd * (1 / 10) >= threshold.
Proof.
  unfold w_qd, threshold. lra.
Qed.

(** The queue weight is not too aggressive: at Q = 0.5 (moderate buildup),
    the contribution (0.25) is still well below the memory weight (5.0),
    so memory pressure still dominates at moderate queue depths. *)
Lemma queue_not_too_aggressive :
  w_qd * (1 / 2) < w_mp * 1.
Proof.
  unfold w_qd, w_mp. lra.
Qed.

(* ================================================================= *)
(** ** Condition 5: Scale-Up from Low N Dominates Mild Memory Pressure *)
(* ================================================================= *)

(** At low thread counts (N = 1→2), the throughput gain is enormous
    (roughly T₁ × (1 − σ) ≈ T₁ × 0.9 for σ = 0.1). After EMA smoothing,
    this dominates even moderate memory pressure (M = 1).

    For this to hold, we need:
      w_tp × delta_T_low × ema_gain > w_mp × M_pressure

    With delta_T_low = T₁ × 0.9 and M_pressure = 1:
      1.0 × T₁ × 0.9 × (0.15/0.85) > 5.0 × 1
      T₁ × 0.1588 > 5.0
      T₁ > 31.5 evals/s

    This holds for nearly any real workload (even trivial programs
    produce 100+ evals/s with a single thread). *)
Theorem scaleup_dominates_pressure : forall T1_val,
  T1_val >= 32 ->
  w_tp * (T1_val * (9 / 10)) * ema_gain > w_mp * 1.
Proof.
  intros T1_val HT1.
  unfold w_tp, w_mp, ema_gain, ema_alpha.
  (* Goal: 1 * (T1_val * (9/10)) * (15/100 / (1 - 15/100)) > 5 * 1 *)
  (* Simplify: T1_val * 9/10 * 15/85 > 5 *)
  (* i.e., T1_val * 135 / 850 > 5 *)
  (* i.e., T1_val * 27 / 170 > 5 *)
  (* i.e., T1_val * 27 > 850 *)
  (* For T1_val >= 32: 32 * 27 = 864 > 850. *)
  assert (H85 : 1 - 15 / 100 > 0) by lra.
  apply Rmult_lt_reg_r with (r := (1 - 15 / 100)).
  - lra.
  - field_simplify; lra.
Qed.

(** At very high throughput gains (e.g., 400 evals/s delta at N = 1→2),
    scale-up dominates even maximum memory pressure (M = 3). *)
Theorem scaleup_dominates_max_pressure :
  w_tp * 400 * ema_gain > w_mp * 3.
Proof.
  unfold w_tp, w_mp, ema_gain, ema_alpha.
  assert (H85 : 1 - 15 / 100 > 0) by lra.
  apply Rmult_lt_reg_r with (r := (1 - 15 / 100)).
  - lra.
  - field_simplify; lra.
Qed.

(* ================================================================= *)
(** ** Combined Dominance Summary *)
(* ================================================================= *)

(** All five weight dominance conditions hold simultaneously. *)
Theorem all_weights_valid :
  (* C1: Memory overrides zero gradient *)
  w_mp > 0 /\
  (* C2: Memory overrides typical near-peak gradient (≤ 9 evals/s) *)
  w_mp * 1 > w_tp * 9 * ema_gain /\
  (* C3: RSS dominates slab *)
  w_rss > w_mp /\
  (* C4: Queue sensitivity matches threshold *)
  w_qd * (1 / 10) >= threshold /\
  (* C5: Scale-up dominates at T₁ ≥ 32 *)
  (forall T1_val, T1_val >= 32 ->
    w_tp * (T1_val * (9 / 10)) * ema_gain > w_mp * 1).
Proof.
  repeat split.
  - unfold w_mp. lra.
  - apply weight_dominance_typical.
  - apply rss_dominates_slab.
  - apply queue_threshold_sensitivity.
  - apply scaleup_dominates_pressure.
Qed.
