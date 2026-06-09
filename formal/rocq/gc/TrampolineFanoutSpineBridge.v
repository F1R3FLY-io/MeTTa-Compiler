(** E3 trampoline fan-out continuation-spine bridge.

    Native trampoline continuations still own the execution state for the hot
    deterministic path, but re-enterable fan-out/collapse frames are exposed to
    the collector through ContinuationAddr-backed bridge nodes. The node walker
    is the GC-facing view of those frames.

    This file proves the implementation-level GC obligation for that bridge:
    every value a future trampoline resume/collapse transition can touch from a
    live fan-out frame is rooted through the bridge node walker. The three
    cut-pruned remaining-branch families are modeled as future touches only
    when their cut barrier has not fired.
*)

Module MeTTaTron_GC_TrampolineFanoutSpineBridge.

Section TrampolineFanoutSpineBridgeModel.
  Variable Frame Addr Value Node : Type.

  Variable LiveFrame : Frame -> Prop.
  Variable BridgeHandle : Frame -> Addr -> Prop.
  Variable Resolves : Addr -> Node -> Prop.
  Variable NodeRoot : Node -> Value -> Prop.

  Variable CutFired : Frame -> Prop.

  Variable RuleMatchRemaining : Frame -> Value -> Prop.
  Variable RuleMatchResult : Frame -> Value -> Prop.
  Variable RuleMatchCurrentBinding : Frame -> Value -> Prop.
  Variable RuleMatchOuterBinding : Frame -> Value -> Prop.

  Variable AmbRemaining : Frame -> Value -> Prop.
  Variable AmbResult : Frame -> Value -> Prop.
  Variable AmbOuterBinding : Frame -> Value -> Prop.

  Variable MatchTemplateRemaining : Frame -> Value -> Prop.
  Variable MatchTemplateResult : Frame -> Value -> Prop.
  Variable MatchTemplateOuterBinding : Frame -> Value -> Prop.

  Variable CollapseRemainingRaw : Frame -> Value -> Prop.
  Variable CollapseEvaluated : Frame -> Value -> Prop.
  Variable CollapseCurrentBinding : Frame -> Value -> Prop.
  Variable CollapseOuterBinding : Frame -> Value -> Prop.

  Variable ParallelInput : Frame -> Value -> Prop.
  Variable ParallelPartialResult : Frame -> Value -> Prop.
  Variable ParallelBaseResult : Frame -> Value -> Prop.
  Variable ParallelOuterBinding : Frame -> Value -> Prop.

  Variable ParallelCollapseInput : Frame -> Value -> Prop.
  Variable ParallelCollapsePartialResult : Frame -> Value -> Prop.
  Variable ParallelCollapseOuterBinding : Frame -> Value -> Prop.

  Definition FutureRuleMatchRemaining (f : Frame) (v : Value) : Prop :=
    ~ CutFired f /\ RuleMatchRemaining f v.

  Definition FutureAmbRemaining (f : Frame) (v : Value) : Prop :=
    ~ CutFired f /\ AmbRemaining f v.

  Definition FutureMatchTemplateRemaining (f : Frame) (v : Value) : Prop :=
    ~ CutFired f /\ MatchTemplateRemaining f v.

  Inductive FutureTrampolineFanoutTouch (f : Frame) (v : Value) : Prop :=
  | future_rule_match_remaining :
      FutureRuleMatchRemaining f v -> FutureTrampolineFanoutTouch f v
  | future_rule_match_result :
      RuleMatchResult f v -> FutureTrampolineFanoutTouch f v
  | future_rule_match_current_binding :
      RuleMatchCurrentBinding f v -> FutureTrampolineFanoutTouch f v
  | future_rule_match_outer_binding :
      RuleMatchOuterBinding f v -> FutureTrampolineFanoutTouch f v
  | future_amb_remaining :
      FutureAmbRemaining f v -> FutureTrampolineFanoutTouch f v
  | future_amb_result :
      AmbResult f v -> FutureTrampolineFanoutTouch f v
  | future_amb_outer_binding :
      AmbOuterBinding f v -> FutureTrampolineFanoutTouch f v
  | future_match_template_remaining :
      FutureMatchTemplateRemaining f v -> FutureTrampolineFanoutTouch f v
  | future_match_template_result :
      MatchTemplateResult f v -> FutureTrampolineFanoutTouch f v
  | future_match_template_outer_binding :
      MatchTemplateOuterBinding f v -> FutureTrampolineFanoutTouch f v
  | future_collapse_remaining_raw :
      CollapseRemainingRaw f v -> FutureTrampolineFanoutTouch f v
  | future_collapse_evaluated :
      CollapseEvaluated f v -> FutureTrampolineFanoutTouch f v
  | future_collapse_current_binding :
      CollapseCurrentBinding f v -> FutureTrampolineFanoutTouch f v
  | future_collapse_outer_binding :
      CollapseOuterBinding f v -> FutureTrampolineFanoutTouch f v
  | future_parallel_input :
      ParallelInput f v -> FutureTrampolineFanoutTouch f v
  | future_parallel_partial_result :
      ParallelPartialResult f v -> FutureTrampolineFanoutTouch f v
  | future_parallel_base_result :
      ParallelBaseResult f v -> FutureTrampolineFanoutTouch f v
  | future_parallel_outer_binding :
      ParallelOuterBinding f v -> FutureTrampolineFanoutTouch f v
  | future_parallel_collapse_input :
      ParallelCollapseInput f v -> FutureTrampolineFanoutTouch f v
  | future_parallel_collapse_partial_result :
      ParallelCollapsePartialResult f v -> FutureTrampolineFanoutTouch f v
  | future_parallel_collapse_outer_binding :
      ParallelCollapseOuterBinding f v -> FutureTrampolineFanoutTouch f v.

  Definition BridgeComplete : Prop :=
    forall f,
      LiveFrame f ->
      exists a n, BridgeHandle f a /\ Resolves a n.

  Definition TrampolineFanoutNodeComplete : Prop :=
    forall f a n v,
      LiveFrame f ->
      BridgeHandle f a ->
      Resolves a n ->
      FutureTrampolineFanoutTouch f v ->
      NodeRoot n v.

  Theorem trampoline_fanout_bridge_roots_future_touch :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      TrampolineFanoutNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall f v,
        LiveFrame f ->
        FutureTrampolineFanoutTouch f v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot f v Hlive Hfuture.
    destruct (Hbridge f Hlive) as [a [n [Hhandle Hresolves]]].
    apply (Hroot n v).
    apply (Hcomplete f a n v Hlive Hhandle Hresolves Hfuture).
  Qed.

  Theorem trampoline_fanout_future_touch_not_freed :
    forall (RootedValue FreedValue : Value -> Prop),
      BridgeComplete ->
      TrampolineFanoutNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      (forall v, FreedValue v -> ~ RootedValue v) ->
      forall f v,
        LiveFrame f ->
        FutureTrampolineFanoutTouch f v ->
        ~ FreedValue v.
  Proof.
    intros RootedValue FreedValue Hbridge Hcomplete Hroot Hfreed
           f v Hlive Hfuture Hfreed_v.
    apply (Hfreed v Hfreed_v).
    apply (trampoline_fanout_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot f v Hlive Hfuture).
  Qed.

  Theorem rule_match_remaining_rooted_when_cut_not_fired :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      TrampolineFanoutNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall f v,
        LiveFrame f ->
        ~ CutFired f ->
        RuleMatchRemaining f v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot f v Hlive Hnot_cut Hremaining.
    apply (trampoline_fanout_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot f v Hlive).
    apply future_rule_match_remaining.
    split; assumption.
  Qed.

  Theorem amb_remaining_rooted_when_cut_not_fired :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      TrampolineFanoutNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall f v,
        LiveFrame f ->
        ~ CutFired f ->
        AmbRemaining f v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot f v Hlive Hnot_cut Hremaining.
    apply (trampoline_fanout_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot f v Hlive).
    apply future_amb_remaining.
    split; assumption.
  Qed.

  Theorem match_template_remaining_rooted_when_cut_not_fired :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      TrampolineFanoutNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall f v,
        LiveFrame f ->
        ~ CutFired f ->
        MatchTemplateRemaining f v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot f v Hlive Hnot_cut Hremaining.
    apply (trampoline_fanout_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot f v Hlive).
    apply future_match_template_remaining.
    split; assumption.
  Qed.

  Theorem cut_fired_rule_match_remaining_occurrence_dead :
    forall f v,
      CutFired f ->
      RuleMatchRemaining f v ->
      ~ FutureRuleMatchRemaining f v.
  Proof.
    intros f v Hcut _ [Hnot_cut _].
    apply Hnot_cut. exact Hcut.
  Qed.

  Theorem cut_fired_amb_remaining_occurrence_dead :
    forall f v,
      CutFired f ->
      AmbRemaining f v ->
      ~ FutureAmbRemaining f v.
  Proof.
    intros f v Hcut _ [Hnot_cut _].
    apply Hnot_cut. exact Hcut.
  Qed.

  Theorem cut_fired_match_template_remaining_occurrence_dead :
    forall f v,
      CutFired f ->
      MatchTemplateRemaining f v ->
      ~ FutureMatchTemplateRemaining f v.
  Proof.
    intros f v Hcut _ [Hnot_cut _].
    apply Hnot_cut. exact Hcut.
  Qed.

  Theorem parallel_partial_result_rooted :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      TrampolineFanoutNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall f v,
        LiveFrame f ->
        ParallelPartialResult f v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot f v Hlive Hpartial.
    apply (trampoline_fanout_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot f v Hlive).
    apply future_parallel_partial_result. exact Hpartial.
  Qed.

  Theorem parallel_collapse_partial_result_rooted :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      TrampolineFanoutNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall f v,
        LiveFrame f ->
        ParallelCollapsePartialResult f v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot f v Hlive Hpartial.
    apply (trampoline_fanout_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot f v Hlive).
    apply future_parallel_collapse_partial_result. exact Hpartial.
  Qed.
End TrampolineFanoutSpineBridgeModel.

End MeTTaTron_GC_TrampolineFanoutSpineBridge.
