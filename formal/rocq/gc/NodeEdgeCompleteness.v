(** Node-edge completeness obligation for the CESK index collector.

    The structural-root theorem is only useful if the marker's implementation
    edge reader covers every semantic heap edge. In the index heap those edge
    classes are inline handle fields, side-arena SExpr/Conjunction children, and
    first-class SpaceHandle contents. State and Memo nodes are leaves by
    construction.
*)

Module MeTTaTron_GC_NodeEdgeCompleteness.

Section NodeEdgeCompletenessModel.
  Variable Addr : Type.

  Definition SemanticEdge
      (InlineEdge SideEdge SpaceEdge : Addr -> Addr -> Prop)
      (parent child : Addr) : Prop :=
    InlineEdge parent child \/ SideEdge parent child \/ SpaceEdge parent child.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Theorem semantic_edge_covered_by_reader :
    forall (InlineEdge SideEdge SpaceEdge ReaderEdge : Addr -> Addr -> Prop),
      (forall parent child, InlineEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SideEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SpaceEdge parent child -> ReaderEdge parent child) ->
      forall parent child,
        SemanticEdge InlineEdge SideEdge SpaceEdge parent child ->
        ReaderEdge parent child.
  Proof.
    intros InlineEdge SideEdge SpaceEdge ReaderEdge
           Hinline Hside Hspace parent child Hedge.
    destruct Hedge as [Hinline_edge | [Hside_edge | Hspace_edge]].
    - apply Hinline. exact Hinline_edge.
    - apply Hside. exact Hside_edge.
    - apply Hspace. exact Hspace_edge.
  Qed.

  Theorem semantic_reachable_seen_by_reader :
    forall (Root Seen : Addr -> Prop)
           (InlineEdge SideEdge SpaceEdge ReaderEdge : Addr -> Addr -> Prop),
      (forall a, Root a -> Seen a) ->
      (forall parent child, Seen parent -> ReaderEdge parent child -> Seen child) ->
      (forall parent child, InlineEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SideEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SpaceEdge parent child -> ReaderEdge parent child) ->
      forall a,
        Reach Root (SemanticEdge InlineEdge SideEdge SpaceEdge) a ->
        Seen a.
  Proof.
    intros Root Seen InlineEdge SideEdge SpaceEdge ReaderEdge
           Hroot Hclosed Hinline Hside Hspace a Hreach.
    induction Hreach as [a Hroot_a | parent child _ IH Hedge].
    - apply Hroot. exact Hroot_a.
    - eapply Hclosed.
      + exact IH.
      + apply (semantic_edge_covered_by_reader
                 InlineEdge SideEdge SpaceEdge ReaderEdge); assumption.
  Qed.

  Theorem semantic_reachable_survives_sweep :
    forall (Root Marked Freed : Addr -> Prop)
           (InlineEdge SideEdge SpaceEdge ReaderEdge : Addr -> Addr -> Prop),
      (forall a, Root a -> Marked a) ->
      (forall parent child, Marked parent -> ReaderEdge parent child -> Marked child) ->
      (forall parent child, InlineEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SideEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SpaceEdge parent child -> ReaderEdge parent child) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        Reach Root (SemanticEdge InlineEdge SideEdge SpaceEdge) a ->
        ~ Freed a.
  Proof.
    intros Root Marked Freed InlineEdge SideEdge SpaceEdge ReaderEdge
           Hroot Hclosed Hinline Hside Hspace Hsweep a Hreach Hfreed.
    apply (Hsweep a Hfreed).
    apply (semantic_reachable_seen_by_reader
             Root Marked InlineEdge SideEdge SpaceEdge ReaderEdge
             Hroot Hclosed Hinline Hside Hspace a).
    exact Hreach.
  Qed.
End NodeEdgeCompletenessModel.

End MeTTaTron_GC_NodeEdgeCompleteness.
