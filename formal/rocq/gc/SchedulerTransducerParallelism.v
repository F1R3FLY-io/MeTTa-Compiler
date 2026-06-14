(** WFST transducer parallelism contract.

    The live scheduler uses the WFST action's [parallelism_degree] as a
    branch-dispatch eligibility gate: degree 1 is sequential, degree > 1 is
    eligible for fanout once the evaluator's purity, depth, pool, and budget
    gates also pass.  This proof pins the transducer-side obligations:

    - the default table never constructs degree 0;
    - the live degree > 1 gate is sound for the two branch-parallel classes;
    - the branch-aware override uses every available branch until the cap;
    - a zero cap degrades to sequential degree 1, not invalid degree 0.
*)

From Stdlib Require Import Arith Lia PeanoNat.

Module MeTTaTron_GC_SchedulerTransducerParallelism.

Inductive CostClass : Type :=
| GroundCheap
| GroundArith
| SymbolicCheap
| SymbolicModerate
| RecursiveBounded
| RecursiveUnbounded
| ParallelPure
| ImpureSequential.

Definition branch_parallel_class (class : CostClass) : Prop :=
  match class with
  | SymbolicModerate | ParallelPure => True
  | _ => False
  end.

Definition default_degree (class : CostClass) : nat :=
  match class with
  | GroundCheap => 1
  | GroundArith => 1
  | SymbolicCheap => 1
  | SymbolicModerate => 4
  | RecursiveBounded => 1
  | RecursiveUnbounded => 1
  | ParallelPure => 8
  | ImpureSequential => 1
  end.

Definition safe_cap (max_parallel : nat) : nat :=
  Nat.max 1 max_parallel.

Definition branch_degree
    (class : CostClass)
    (branch_count max_parallel : nat) : nat :=
  match class with
  | SymbolicModerate | ParallelPure =>
      if 1 <? branch_count then
        Nat.min branch_count (safe_cap max_parallel)
      else
        default_degree class
  | _ => default_degree class
  end.

Theorem safe_cap_positive :
  forall max_parallel,
    1 <= safe_cap max_parallel.
Proof.
  unfold safe_cap.
  lia.
Qed.

Theorem default_degree_positive :
  forall class,
    1 <= default_degree class.
Proof.
  destruct class; simpl; lia.
Qed.

Theorem non_branch_classes_default_sequential :
  forall class,
    ~ branch_parallel_class class ->
    default_degree class = 1.
Proof.
  destruct class; simpl; intros Hnot; try reflexivity;
    exfalso; apply Hnot; exact I.
Qed.

Theorem default_gate_sound :
  forall class,
    1 < default_degree class ->
    branch_parallel_class class.
Proof.
  destruct class; simpl; intros Hdegree; try lia; exact I.
Qed.

Theorem branch_degree_positive :
  forall class branch_count max_parallel,
    1 <= branch_degree class branch_count max_parallel.
Proof.
  intros class branch_count max_parallel.
  destruct class; simpl; try lia.
  - destruct (1 <? branch_count) eqn:Hbranch; try lia.
    apply Nat.ltb_lt in Hbranch;
    apply Nat.min_glb.
    + lia.
    + apply safe_cap_positive.
  - destruct (1 <? branch_count) eqn:Hbranch; try lia.
    apply Nat.ltb_lt in Hbranch;
    apply Nat.min_glb.
    + lia.
    + apply safe_cap_positive.
Qed.

Theorem branch_degree_respects_safe_cap :
  forall class branch_count max_parallel,
    branch_parallel_class class ->
    1 < branch_count ->
    branch_degree class branch_count max_parallel <= safe_cap max_parallel.
Proof.
  destruct class; simpl; intros branch_count max_parallel Hclass Hbranches;
    try contradiction;
    assert (Hbranch : (1 <? branch_count) = true) by (apply Nat.ltb_lt; lia);
    rewrite Hbranch;
    apply Nat.le_min_r.
Qed.

Theorem branch_degree_maximal_before_cap :
  forall class branch_count max_parallel,
    branch_parallel_class class ->
    1 < branch_count ->
    branch_count <= safe_cap max_parallel ->
    branch_degree class branch_count max_parallel = branch_count.
Proof.
  intros class branch_count max_parallel Hclass Hbranches Hcap.
  destruct class; simpl in *; try contradiction.
  - assert (Hbranch : (1 <? branch_count) = true) by (apply Nat.ltb_lt; lia).
    rewrite Hbranch;
    apply Nat.min_l.
    exact Hcap.
  - assert (Hbranch : (1 <? branch_count) = true) by (apply Nat.ltb_lt; lia).
    rewrite Hbranch;
    apply Nat.min_l.
    exact Hcap.
Qed.

Theorem zero_cap_degrades_to_sequential_nonzero :
  forall class branch_count,
    branch_parallel_class class ->
    1 < branch_count ->
    branch_degree class branch_count 0 = 1.
Proof.
  destruct class; simpl; intros branch_count Hclass Hbranches;
    try contradiction;
    assert (Hbranch : (1 <? branch_count) = true) by (apply Nat.ltb_lt; lia);
    rewrite Hbranch;
    apply Nat.min_r;
    lia.
Qed.

Theorem non_branch_classes_ignore_branch_inputs :
  forall class branch_count max_parallel,
    ~ branch_parallel_class class ->
    branch_degree class branch_count max_parallel = default_degree class.
Proof.
  destruct class; simpl; intros branch_count max_parallel Hnot; try reflexivity;
    exfalso; apply Hnot; exact I.
Qed.

Theorem branch_degree_gate_sound_with_zero_cap :
  forall class branch_count,
    branch_parallel_class class ->
    1 < branch_count ->
    ~ branch_degree class branch_count 0 = 0.
Proof.
  intros class branch_count Hclass Hbranches Hzero.
  pose proof
    (zero_cap_degrades_to_sequential_nonzero class branch_count Hclass Hbranches)
    as Hseq.
  lia.
Qed.

End MeTTaTron_GC_SchedulerTransducerParallelism.
