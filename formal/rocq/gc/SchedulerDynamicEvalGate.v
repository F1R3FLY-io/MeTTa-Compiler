(** Dynamic evaluation parallel-dispatch gate.

    The scheduler's no-budget parallel path uses a syntactic blocker to rule out
    bodies whose execution can mutate evaluator state. Dynamic evaluation heads
    (`eval`, `!`, `evalc`) can execute code carried by a variable or another
    expression, so absence of a visible state-mutating head is not enough.
*)

Module MeTTaTron_GC_SchedulerDynamicEvalGate.

Section DynamicEvalGateModel.
  Definition blocks_parallel_dispatch
      (dynamic_eval_gate dynamic_eval state_mutation strict_print io : Prop)
      : Prop :=
    state_mutation \/
    (strict_print /\ io) \/
    (dynamic_eval_gate /\ dynamic_eval).

  Definition no_budget_parallel_allowed
      (dynamic_eval_gate dynamic_eval state_mutation strict_print io : Prop)
      : Prop :=
    ~ blocks_parallel_dispatch
        dynamic_eval_gate dynamic_eval state_mutation strict_print io.

  Theorem present_dynamic_eval_gate_blocks_dynamic_eval :
    forall dynamic_eval_gate dynamic_eval state_mutation strict_print io,
      dynamic_eval_gate ->
      dynamic_eval ->
      blocks_parallel_dispatch
        dynamic_eval_gate dynamic_eval state_mutation strict_print io.
  Proof.
    intros dynamic_eval_gate dynamic_eval state_mutation strict_print io
      Hgate Heval.
    unfold blocks_parallel_dispatch.
    right.
    right.
    split.
    - exact Hgate.
    - exact Heval.
  Qed.

  Theorem dynamic_eval_gate_blocks_dynamic_eval :
    forall dynamic_eval state_mutation strict_print io,
      dynamic_eval ->
      blocks_parallel_dispatch
        True dynamic_eval state_mutation strict_print io.
  Proof.
    intros dynamic_eval state_mutation strict_print io Heval.
    apply present_dynamic_eval_gate_blocks_dynamic_eval.
    - exact I.
    - exact Heval.
  Qed.

  Theorem no_budget_parallel_excludes_gated_dynamic_eval :
    forall dynamic_eval_gate dynamic_eval state_mutation strict_print io,
      no_budget_parallel_allowed
        dynamic_eval_gate dynamic_eval state_mutation strict_print io ->
      dynamic_eval_gate ->
      ~ dynamic_eval.
  Proof.
    intros dynamic_eval_gate dynamic_eval state_mutation strict_print io
      Hallowed Hgate Heval.
    unfold no_budget_parallel_allowed in Hallowed.
    apply Hallowed.
    apply present_dynamic_eval_gate_blocks_dynamic_eval; assumption.
  Qed.

  Theorem no_budget_parallel_excludes_dynamic_eval :
    forall dynamic_eval state_mutation strict_print io,
      no_budget_parallel_allowed
        True dynamic_eval state_mutation strict_print io ->
      ~ dynamic_eval.
  Proof.
    intros dynamic_eval state_mutation strict_print io Hallowed.
    eapply
      (no_budget_parallel_excludes_gated_dynamic_eval
        True dynamic_eval state_mutation strict_print io).
    - exact Hallowed.
    - exact I.
  Qed.

  Theorem present_state_mutation_blocks :
    forall dynamic_eval_gate dynamic_eval state_mutation strict_print io,
      state_mutation ->
      blocks_parallel_dispatch
        dynamic_eval_gate dynamic_eval state_mutation strict_print io.
  Proof.
    intros dynamic_eval_gate dynamic_eval state_mutation strict_print io
      Hstate.
    unfold blocks_parallel_dispatch.
    left.
    exact Hstate.
  Qed.

  Theorem state_mutation_still_blocks :
    forall dynamic_eval_gate dynamic_eval strict_print io,
      blocks_parallel_dispatch
        dynamic_eval_gate dynamic_eval True strict_print io.
  Proof.
    intros dynamic_eval_gate dynamic_eval strict_print io.
    apply present_state_mutation_blocks.
    exact I.
  Qed.

  Theorem present_strict_io_blocks :
    forall dynamic_eval_gate dynamic_eval state_mutation strict_print io,
      strict_print ->
      io ->
      blocks_parallel_dispatch
        dynamic_eval_gate dynamic_eval state_mutation strict_print io.
  Proof.
    intros dynamic_eval_gate dynamic_eval state_mutation strict_print io
      Hstrict Hio.
    unfold blocks_parallel_dispatch.
    right.
    left.
    split.
    - exact Hstrict.
    - exact Hio.
  Qed.

  Theorem strict_io_still_blocks :
    forall dynamic_eval_gate dynamic_eval state_mutation,
      blocks_parallel_dispatch
        dynamic_eval_gate dynamic_eval state_mutation True True.
  Proof.
    intros dynamic_eval_gate dynamic_eval state_mutation.
    apply present_strict_io_blocks; exact I.
  Qed.

  Theorem missing_dynamic_eval_gate_allows_counterexample :
    no_budget_parallel_allowed False True False False False.
  Proof.
    unfold no_budget_parallel_allowed, blocks_parallel_dispatch.
    intros [Hstate | [[Hstrict _] | [Hgate _]]].
    - exact Hstate.
    - exact Hstrict.
    - exact Hgate.
  Qed.
End DynamicEvalGateModel.

End MeTTaTron_GC_SchedulerDynamicEvalGate.
