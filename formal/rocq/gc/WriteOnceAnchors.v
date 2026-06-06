(** Write-once global-anchor rooting obligation.

    Some E0 roots are process-global write-once anchors.  For the bytecode
    compiler's cached atoms, the implementation has exactly three
    `OnceLock<MettaValue>` cells and `collect_compiler_atom_roots` reads each
    initialized cell structurally.  The source-coupling harness pins both the
    reader and the absence of reset/take deletion paths.  This proof captures
    the abstract safety shape used by those anchors.
*)

Module MeTTaTron_GC_WriteOnceAnchors.

Section WriteOnceAnchorModel.
  Variables Anchor Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition WriteOnceAnchorLive
      (Initialized Deleted : Anchor -> Prop)
      (slot : Anchor) : Prop :=
    Initialized slot /\ ~ Deleted slot.

  Theorem write_once_anchor_scanned :
    forall (Initialized Deleted Scanned : Anchor -> Prop),
      (forall slot, Initialized slot -> ~ Deleted slot) ->
      (forall slot, WriteOnceAnchorLive Initialized Deleted slot -> Scanned slot) ->
      forall slot,
        Initialized slot ->
        Scanned slot.
  Proof.
    intros Initialized Deleted Scanned Hnot_deleted Hscanned slot Hinit.
    apply Hscanned.
    split.
    - exact Hinit.
    - apply Hnot_deleted.
      exact Hinit.
  Qed.

  Theorem write_once_anchor_rooted :
    forall (Initialized Deleted Scanned : Anchor -> Prop)
      (AnchorValue : Anchor -> Addr -> Prop)
      (StructuralRoot : Addr -> Prop),
      (forall slot, Initialized slot -> ~ Deleted slot) ->
      (forall slot, WriteOnceAnchorLive Initialized Deleted slot -> Scanned slot) ->
      (forall slot a, Scanned slot -> AnchorValue slot a -> StructuralRoot a) ->
      forall slot a,
        Initialized slot ->
        AnchorValue slot a ->
        StructuralRoot a.
  Proof.
    intros Initialized Deleted Scanned AnchorValue StructuralRoot
           Hnot_deleted Hscanned Hroot slot a Hinit Hvalue.
    apply (Hroot slot a).
    - eapply write_once_anchor_scanned; eauto.
    - exact Hvalue.
  Qed.

  Theorem write_once_anchor_survives_collection :
    forall (Initialized Deleted Scanned : Anchor -> Prop)
           (AnchorValue : Anchor -> Addr -> Prop)
      (StructuralRoot Marked Freed : Addr -> Prop)
      (Edge : Addr -> Addr -> Prop),
      (forall slot, Initialized slot -> ~ Deleted slot) ->
      (forall slot, WriteOnceAnchorLive Initialized Deleted slot -> Scanned slot) ->
      (forall slot a, Scanned slot -> AnchorValue slot a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall slot a,
        Initialized slot ->
        AnchorValue slot a ->
        ~ Freed a.
  Proof.
    intros Initialized Deleted Scanned AnchorValue StructuralRoot Marked Freed Edge
           Hnot_deleted Hscanned Hroot Hmark Hsweep slot a Hinit Hvalue Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    eapply write_once_anchor_rooted; eauto.
  Qed.
End WriteOnceAnchorModel.

End MeTTaTron_GC_WriteOnceAnchors.
