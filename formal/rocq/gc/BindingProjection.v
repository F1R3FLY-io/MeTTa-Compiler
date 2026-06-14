(** Binding-sidecar projection for tracked parallel branch results.

    A ProcessRuleMatches branch may compose a caller-visible result with a
    large sidecar binding map.  At an explicit tracked binding boundary, the
    implementation projects that sidecar to the transitive variables still
    observable from the result value and from the tracked caller variables.

    This model captures the proof obligation used by the source change.  When
    a tracked/consumer liveness context exists, projection is a narrowing of the
    original binding map, it retains every live/observable key, it drops stale
    freshened keys, and it cannot leave a visible binding pointing at a dropped
    bound freshened key when the live set is closed under visible references.
    When no such context exists, the implementation deliberately defers the
    projection and preserves the original sidecar; stale-key dropping is not an
    obligation at that boundary because later fold/progn consumers may still
    need bindings that are not syntactically live in the immediate value.
*)

Module MeTTaTron_GC_BindingProjection.

Section BindingProjectionModel.
  Variable Var : Type.

  Definition VarSet := Var -> Prop.

  Definition subset (a b : VarSet) : Prop :=
    forall x, a x -> b x.

  Definition BindingSource (Source : VarSet) (x : Var) : Prop :=
    Source x.

  Definition Projected (Source Keep : VarSet) (x : Var) : Prop :=
    Source x /\ Keep x.

  Definition EffectiveProjected
      (HasProjectionContext : bool)
      (Source Keep : VarSet)
      (x : Var) : Prop :=
    if HasProjectionContext then Projected Source Keep x else Source x.

  Definition FreshStale (Fresh Keep : VarSet) (x : Var) : Prop :=
    Fresh x /\ ~ Keep x.

  Definition VisibleReferenceClosed
      (Visible RefersTo Keep : VarSet)
      (Edge : Var -> Var -> Prop) : Prop :=
    forall visible referenced,
      Visible visible ->
      Keep visible ->
      Edge visible referenced ->
      RefersTo referenced ->
      Keep referenced.

  Theorem projection_subset_source :
    forall (Source Keep : VarSet),
      subset (Projected Source Keep) Source.
  Proof.
    intros Source Keep x Hprojected.
    destruct Hprojected as [Hsource _].
    exact Hsource.
  Qed.

  Theorem projection_retains_live_keys :
    forall (Source Keep Live : VarSet),
      subset Live Source ->
      subset Live Keep ->
      subset Live (Projected Source Keep).
  Proof.
    intros Source Keep Live Hlive_source Hlive_keep x Hlive.
    split.
    - apply Hlive_source. exact Hlive.
    - apply Hlive_keep. exact Hlive.
  Qed.

  Theorem stale_fresh_key_not_projected :
    forall (Source Keep Fresh : VarSet) x,
      FreshStale Fresh Keep x ->
      ~ Projected Source Keep x.
  Proof.
    intros Source Keep Fresh x Hstale Hprojected.
    destruct Hstale as [_ Hnot_keep].
    destruct Hprojected as [_ Hkeep].
    apply Hnot_keep.
    exact Hkeep.
  Qed.

  Theorem effective_projection_with_context_is_projected :
    forall (Source Keep : VarSet),
      subset (EffectiveProjected true Source Keep) (Projected Source Keep).
  Proof.
    intros Source Keep x Heffective.
    exact Heffective.
  Qed.

  Theorem effective_projection_without_context_is_source :
    forall (Source Keep : VarSet),
      subset Source (EffectiveProjected false Source Keep).
  Proof.
    intros Source Keep x Hsource.
    exact Hsource.
  Qed.

  Theorem no_context_defers_stale_fresh_dropping :
    forall (Source Keep Fresh : VarSet) x,
      Source x ->
      FreshStale Fresh Keep x ->
      EffectiveProjected false Source Keep x.
  Proof.
    intros Source Keep Fresh x Hsource _.
    exact Hsource.
  Qed.

  Theorem visible_reference_target_retained :
    forall (Source Keep Visible RefersTo : VarSet)
           (Edge : Var -> Var -> Prop)
           (visible referenced : Var),
      VisibleReferenceClosed Visible RefersTo Keep Edge ->
      Projected Source Keep visible ->
      Visible visible ->
      Edge visible referenced ->
      RefersTo referenced ->
      Keep referenced.
  Proof.
    intros Source Keep Visible RefersTo Edge visible referenced
           Hclosed Hprojected Hvisible Hedge Hrefers.
    destruct Hprojected as [_ Hkeep_visible].
    apply (Hclosed visible referenced).
    - exact Hvisible.
    - exact Hkeep_visible.
    - exact Hedge.
    - exact Hrefers.
  Qed.

  Theorem projected_visible_reference_not_dangling :
    forall (Source Keep Visible RefersTo BoundFresh : VarSet)
           (Edge : Var -> Var -> Prop)
           (visible referenced : Var),
      VisibleReferenceClosed Visible RefersTo Keep Edge ->
      Projected Source Keep visible ->
      Visible visible ->
      Edge visible referenced ->
      RefersTo referenced ->
      BoundFresh referenced ->
      ~ (BoundFresh referenced /\ ~ Keep referenced).
  Proof.
    intros Source Keep Visible RefersTo BoundFresh Edge visible referenced
           Hclosed Hprojected Hvisible Hedge Hrefers Hbound Hbad.
    destruct Hbad as [_ Hnot_keep].
    apply Hnot_keep.
    apply (visible_reference_target_retained
             Source Keep Visible RefersTo Edge visible referenced).
    - exact Hclosed.
    - exact Hprojected.
    - exact Hvisible.
    - exact Hedge.
    - exact Hrefers.
  Qed.

  Theorem tracked_projection_sound :
    forall (Source Keep Live Fresh Visible RefersTo BoundFresh : VarSet)
           (Edge : Var -> Var -> Prop),
      subset Live Source ->
      subset Live Keep ->
      VisibleReferenceClosed Visible RefersTo Keep Edge ->
      subset Live (Projected Source Keep)
      /\ (forall x, FreshStale Fresh Keep x -> ~ Projected Source Keep x)
      /\ (forall visible referenced,
            Projected Source Keep visible ->
            Visible visible ->
            Edge visible referenced ->
            RefersTo referenced ->
            BoundFresh referenced ->
            ~ (BoundFresh referenced /\ ~ Keep referenced)).
  Proof.
    intros Source Keep Live Fresh Visible RefersTo BoundFresh Edge
           Hlive_source Hlive_keep Hclosed.
    split.
    - apply projection_retains_live_keys.
      + exact Hlive_source.
      + exact Hlive_keep.
    - split.
      + intros x Hstale.
        apply (stale_fresh_key_not_projected Source Keep Fresh).
        exact Hstale.
      + intros visible referenced Hprojected Hvisible Hedge Hrefers Hbound Hbad.
        apply (projected_visible_reference_not_dangling
                 Source Keep Visible RefersTo BoundFresh Edge visible referenced).
        * exact Hclosed.
        * exact Hprojected.
        * exact Hvisible.
        * exact Hedge.
        * exact Hrefers.
        * exact Hbound.
        * exact Hbad.
  Qed.

  Theorem binding_projection_context_partition :
    forall (Source Keep Live Fresh Visible RefersTo BoundFresh : VarSet)
           (Edge : Var -> Var -> Prop),
      subset Live Source ->
      subset Live Keep ->
      VisibleReferenceClosed Visible RefersTo Keep Edge ->
      (subset Live (EffectiveProjected true Source Keep)
       /\ (forall x,
             FreshStale Fresh Keep x ->
             ~ EffectiveProjected true Source Keep x)
       /\ (forall visible referenced,
             EffectiveProjected true Source Keep visible ->
             Visible visible ->
             Edge visible referenced ->
             RefersTo referenced ->
             BoundFresh referenced ->
             ~ (BoundFresh referenced /\ ~ Keep referenced)))
      /\ subset Source (EffectiveProjected false Source Keep).
  Proof.
    intros Source Keep Live Fresh Visible RefersTo BoundFresh Edge
           Hlive_source Hlive_keep Hclosed.
    split.
    - destruct (tracked_projection_sound
                  Source Keep Live Fresh Visible RefersTo BoundFresh Edge
                  Hlive_source Hlive_keep Hclosed)
        as [Hlive_projected [Hstale_not_projected Hdangling]].
      split.
      + intros x Hlive.
        apply Hlive_projected.
        exact Hlive.
      + split.
        * intros x Hstale Heffective.
          apply (Hstale_not_projected x Hstale).
          exact Heffective.
        * intros visible referenced Heffective Hvisible Hedge Hrefers Hbound Hbad.
          apply (Hdangling visible referenced).
          -- exact Heffective.
          -- exact Hvisible.
          -- exact Hedge.
          -- exact Hrefers.
          -- exact Hbound.
          -- exact Hbad.
    - apply effective_projection_without_context_is_source.
  Qed.
End BindingProjectionModel.

End MeTTaTron_GC_BindingProjection.
