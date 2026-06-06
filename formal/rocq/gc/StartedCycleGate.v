(** Rocq model of the E5 started-cycle straddle gate obligation.

    `GC_CYCLE_GEN` is bumped during teardown before a new driver has actually
    started the next cycle.  A straddling worker must therefore gate re-park on
    `GC_CYCLE_STARTED > my_reparked_gen`, not merely on the generation counter.
*)

Module MeTTaTron_GC_StartedCycleGate.

Section StartedCycleGateModel.
  Theorem started_gate_prevents_phantom_repark :
    forall (StartedAfterMy Repark Phantom : Prop),
      (Repark -> StartedAfterMy) ->
      (Phantom -> Repark) ->
      (Phantom -> ~ StartedAfterMy) ->
      ~ Phantom.
  Proof.
    intros StartedAfterMy Repark Phantom Hgate Hphantom_repark Hphantom_stale Hphantom.
    apply (Hphantom_stale Hphantom).
    apply Hgate.
    apply Hphantom_repark.
    exact Hphantom.
  Qed.

  Theorem real_started_cycle_repark_is_not_phantom :
    forall (StartedAfterMy Repark Phantom : Prop),
      (StartedAfterMy -> Repark) ->
      (Phantom -> ~ StartedAfterMy) ->
      StartedAfterMy ->
      Repark /\ ~ Phantom.
  Proof.
    intros StartedAfterMy Repark Phantom Hrepark Hphantom_stale Hstarted.
    split.
    - apply Hrepark. exact Hstarted.
    - intro Hphantom.
      apply (Hphantom_stale Hphantom).
      exact Hstarted.
  Qed.
End StartedCycleGateModel.

End MeTTaTron_GC_StartedCycleGate.
