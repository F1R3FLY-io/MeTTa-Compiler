(** Rocq model of the cross-cycle witness-ok reset obligation.

    `CURRENT_WITNESS_OK` is a non-generational boolean gate.  Cycle teardown
    must clear it before the next cycle can observe the gate; otherwise a stale
    true value from the previous cycle could allow collection before the next
    cycle's witness wait has established root completeness.
*)

Module MeTTaTron_GC_WitnessOkReset.

Section WitnessOkResetModel.
  Theorem cleared_witness_ok_blocks_collection :
    forall (WitnessOk Collect : Prop),
      ~ WitnessOk ->
      (Collect -> WitnessOk) ->
      ~ Collect.
  Proof.
    intros WitnessOk Collect Hcleared Hgate Hcollect.
    apply Hcleared.
    apply Hgate.
    exact Hcollect.
  Qed.

  Theorem fresh_witness_ok_prevents_stale_collect :
    forall (WitnessOk FreshWitness Collect StaleCollect : Prop),
      (Collect -> WitnessOk) ->
      (WitnessOk -> FreshWitness) ->
      (FreshWitness -> ~ StaleCollect) ->
      Collect ->
      ~ StaleCollect.
  Proof.
    intros WitnessOk FreshWitness Collect StaleCollect
           Hgate Hfresh Hfresh_not_stale Hcollect.
    apply Hfresh_not_stale.
    apply Hfresh.
    apply Hgate.
    exact Hcollect.
  Qed.
End WitnessOkResetModel.

End MeTTaTron_GC_WitnessOkReset.
