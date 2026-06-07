(** Generic CESK structural-root safety theorem in Rocq.

    Roots are the CESK machine registers and persistent anchors. If marking is
    complete over their reachability closure, sweep frees only unmarked nodes,
    and future transitions touch only structurally reachable addresses, no
    future touch can be freed.
*)

Module MeTTaTron_GC_StructuralRoots.

Section StructuralRootsModel.
  Variable Addr : Type.

  Record Registers : Type := {
    control : Addr -> Prop;
    env : Addr -> Prop;
    kont : Addr -> Prop;
    global : Addr -> Prop;
  }.

  Definition StructuralRoot (regs : Registers) (a : Addr) : Prop :=
    control regs a \/ env regs a \/ kont regs a \/ global regs a.

  Inductive RootSource : Type :=
  | StructuralSource : RootSource
  | DriverSource : RootSource
  | RegistrySource : RootSource.

  Definition IndexRootSource (source : RootSource) : Prop :=
    match source with
    | StructuralSource => True
    | DriverSource => True
    | RegistrySource => False
    end.

  Definition IndexCollectorRoot
      (Structural Driver : Addr -> Prop)
      (a : Addr) : Prop :=
    Structural a \/ Driver a.

  Theorem registry_is_not_index_root_source :
    ~ IndexRootSource RegistrySource.
  Proof.
    intro Hsource.
    exact Hsource.
  Qed.

  Theorem index_root_source_not_registry :
    forall source,
      IndexRootSource source ->
      source <> RegistrySource.
  Proof.
    intros source Hsource Hregistry.
    destruct source.
    - discriminate Hregistry.
    - discriminate Hregistry.
    - exact Hsource.
  Qed.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Theorem structural_root_is_reachable :
    forall (regs : Registers) (Edge : Addr -> Addr -> Prop) (a : Addr),
      StructuralRoot regs a -> Reach (StructuralRoot regs) Edge a.
  Proof.
    intros regs Edge a Hroot.
    apply reach_root.
    exact Hroot.
  Qed.

  Theorem no_future_touch_uaf :
    forall (regs : Registers)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed FutureTouch : Addr -> Prop),
      (forall a, Reach (StructuralRoot regs) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a, FutureTouch a -> Reach (StructuralRoot regs) Edge a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros regs Edge Marked Freed FutureTouch Hmark Hsweep Hfuture a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.

  Theorem registry_independent_no_future_touch_uaf :
    forall (Structural Driver Registry Marked Freed FutureTouch : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, Reach (IndexCollectorRoot Structural Driver) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a, FutureTouch a -> Reach (IndexCollectorRoot Structural Driver) Edge a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros Structural Driver Registry Marked Freed FutureTouch Edge
           Hmark Hsweep Hfuture a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.

  Theorem reachable_node_survives_sweep :
    forall (regs : Registers)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, Reach (StructuralRoot regs) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, Reach (StructuralRoot regs) Edge a -> ~ Freed a.
  Proof.
    intros regs Edge Marked Freed Hmark Hsweep a Hreach Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    exact Hreach.
  Qed.
End StructuralRootsModel.

End MeTTaTron_GC_StructuralRoots.
