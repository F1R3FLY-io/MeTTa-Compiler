(** Frame-local environment roots for forked CESK frames.

    Forked nondeterministic environments can carry values in CoW-local maps
    that are not reachable from E0. The index collector must therefore read
    those maps from every live work item and continuation frame before
    publishing the mutator's thread contribution.
*)

Module MeTTaTron_GC_FrameEnvRoots.

Section FrameEnvRootsModel.
  Variable Addr : Type.

  Definition ForkLocalRoot
      (Binding TypeAssertion StateCell NamedSpace InferredType : Addr -> Prop)
      (a : Addr) : Prop :=
    Binding a \/ TypeAssertion a \/ StateCell a \/ NamedSpace a \/ InferredType a.

  Theorem fork_local_component_in_frame_root :
    forall (Binding TypeAssertion StateCell NamedSpace InferredType
            FrameRoot : Addr -> Prop),
      (forall a, Binding a -> FrameRoot a) ->
      (forall a, TypeAssertion a -> FrameRoot a) ->
      (forall a, StateCell a -> FrameRoot a) ->
      (forall a, NamedSpace a -> FrameRoot a) ->
      (forall a, InferredType a -> FrameRoot a) ->
      forall a,
        ForkLocalRoot Binding TypeAssertion StateCell NamedSpace InferredType a ->
        FrameRoot a.
  Proof.
    intros Binding TypeAssertion StateCell NamedSpace InferredType FrameRoot
           Hbinding Htype Hstate Hnamed Hinferred a Hroot.
    destruct Hroot as [Hbinding_a | [Htype_a | [Hstate_a | [Hnamed_a | Hinferred_a]]]].
    - apply Hbinding. exact Hbinding_a.
    - apply Htype. exact Htype_a.
    - apply Hstate. exact Hstate_a.
    - apply Hnamed. exact Hnamed_a.
    - apply Hinferred. exact Hinferred_a.
  Qed.

  Theorem fork_local_root_survives_collection :
    forall (Binding TypeAssertion StateCell NamedSpace InferredType
            FrameRoot ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop),
      (forall a, Binding a -> FrameRoot a) ->
      (forall a, TypeAssertion a -> FrameRoot a) ->
      (forall a, StateCell a -> FrameRoot a) ->
      (forall a, NamedSpace a -> FrameRoot a) ->
      (forall a, InferredType a -> FrameRoot a) ->
      (forall a, FrameRoot a -> ThreadRoot a) ->
      (forall a, ThreadRoot a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        ForkLocalRoot Binding TypeAssertion StateCell NamedSpace InferredType a ->
        ~ Freed a.
  Proof.
    intros Binding TypeAssertion StateCell NamedSpace InferredType
           FrameRoot ThreadRoot BufferRoot DriverRoot Marked Freed
           Hbinding Htype Hstate Hnamed Hinferred
           Hframe Hpublish Hdrain Hmark Hsweep a Hfork Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hdrain.
    apply Hpublish.
    apply Hframe.
    apply (fork_local_component_in_frame_root
             Binding TypeAssertion StateCell NamedSpace InferredType FrameRoot
             Hbinding Htype Hstate Hnamed Hinferred a).
    exact Hfork.
  Qed.
End FrameEnvRootsModel.

End MeTTaTron_GC_FrameEnvRoots.
