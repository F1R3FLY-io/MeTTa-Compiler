(** Rocq obligation for non-rendezvous index-GC gates.

    True-quiescence collection has no live evaluator/native stack, so fanout
    configuration does not by itself make it unsafe.  Midloop collection is
    different: when fanout is configured, a non-rendezvous midloop sweep can
    miss future/current branch control and must be blocked in favor of the
    rendezvous witness protocol.
*)

Module MeTTaTron_GC_NonRendezvousFanoutGate.

Section NonRendezvousFanoutGateModel.
  Variables IndexMode FanoutEnabled NeverSpawned ActiveZero ActiveOne NThreadsZero Disabled : Prop.

  Definition QuiescenceGate : Prop :=
    IndexMode /\ ActiveZero /\ NThreadsZero /\ ~ Disabled.

  Definition MidloopNonRendezvousGate : Prop :=
    IndexMode /\ ~ FanoutEnabled /\ NeverSpawned /\ ActiveOne /\ ~ Disabled.

  Theorem midloop_non_rendezvous_gate_excludes_fanout :
    MidloopNonRendezvousGate -> ~ FanoutEnabled.
  Proof.
    intros Hgate.
    destruct Hgate as [_ [Hnot_fanout _]].
    exact Hnot_fanout.
  Qed.

  Theorem fanout_enabled_blocks_midloop_non_rendezvous :
    FanoutEnabled -> ~ MidloopNonRendezvousGate.
  Proof.
    intros Hfanout Hgate.
    apply (midloop_non_rendezvous_gate_excludes_fanout Hgate).
    exact Hfanout.
  Qed.

  Theorem fanout_enabled_does_not_block_true_quiescence :
    IndexMode ->
    ActiveZero ->
    NThreadsZero ->
    ~ Disabled ->
    QuiescenceGate.
  Proof.
    intros Hindex Hactive Hthreads Hdisabled.
    unfold QuiescenceGate.
    repeat split; assumption.
  Qed.

  Theorem fanout_zero_single_thread_conditions_open_midloop_gate :
    IndexMode ->
    ~ FanoutEnabled ->
    NeverSpawned ->
    ActiveOne ->
    ~ Disabled ->
    MidloopNonRendezvousGate.
  Proof.
    intros Hindex Hfanout Hspawned Hactive Hdisabled.
    unfold MidloopNonRendezvousGate.
    repeat split; assumption.
  Qed.
End NonRendezvousFanoutGateModel.

End MeTTaTron_GC_NonRendezvousFanoutGate.
