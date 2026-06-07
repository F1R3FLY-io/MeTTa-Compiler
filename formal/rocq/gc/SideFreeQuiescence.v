(** Side-payload free quiescence obligation.

    Index nodes may materialize laundered references into side-arena boxes.
    Reclaiming the fixed node slot is independent from dropping those side
    boxes: side boxes may be dropped only on the true-quiescence arm, and the
    per-thread materialization shadow must be cleared before any later
    dereference can observe a dropped box.
*)

Module MeTTaTron_GC_SideFreeQuiescence.

Section SideFreeQuiescenceModel.
  Variables Quiescent SideFreed StackLaunderLive ShadowCleared FutureDeref : Prop.

  Theorem side_free_at_quiescence_has_no_stack_launder_ref :
    (SideFreed -> Quiescent) ->
    (StackLaunderLive -> ~ Quiescent) ->
    SideFreed ->
    ~ StackLaunderLive.
  Proof.
    intros Hfree_quiescent Hstack_nonquiescent Hfreed Hstack.
    apply (Hstack_nonquiescent Hstack).
    apply Hfree_quiescent.
    exact Hfreed.
  Qed.

  Theorem side_free_shadow_clear_blocks_future_deref :
    (SideFreed -> Quiescent) ->
    (StackLaunderLive -> ~ Quiescent) ->
    (SideFreed -> ShadowCleared) ->
    (FutureDeref -> StackLaunderLive \/ ~ ShadowCleared) ->
    SideFreed ->
    ~ FutureDeref.
  Proof.
    intros Hfree_quiescent Hstack_nonquiescent Hclear Hderef_shape Hfreed Hderef.
    destruct (Hderef_shape Hderef) as [Hstack | Hnot_cleared].
    - apply (side_free_at_quiescence_has_no_stack_launder_ref
               Hfree_quiescent Hstack_nonquiescent Hfreed).
      exact Hstack.
    - apply Hnot_cleared.
      apply Hclear.
      exact Hfreed.
  Qed.

  Theorem nonquiescent_collection_defers_side_free :
    (SideFreed -> Quiescent) ->
    ~ Quiescent ->
    ~ SideFreed.
  Proof.
    intros Hfree_quiescent Hnot_quiescent Hfreed.
    apply Hnot_quiescent.
    apply Hfree_quiescent.
    exact Hfreed.
  Qed.
End SideFreeQuiescenceModel.

End MeTTaTron_GC_SideFreeQuiescence.
