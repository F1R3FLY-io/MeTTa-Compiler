(** E4 serializable continuation slice safety.

    A serialized suspended CESK state is safe to restore only when the stored
    slice contains the full transitive store closure of its control,
    environment, and continuation roots.  The theorem below is intentionally
    independent of any concrete serializer format: it captures the obligation
    that every address a restored transition can touch is present in the
    serialized slice.
*)

Module MeTTaTron_GC_SerializableContinuationSlice.

Section SerializableContinuationSliceModel.
  Variable Addr : Type.

  Inductive Reach (Seed : Addr -> Prop) (Edge : Addr -> Addr -> Prop)
      : Addr -> Prop :=
  | reach_seed : forall a, Seed a -> Reach Seed Edge a
  | reach_step :
      forall a b, Reach Seed Edge a -> Edge a b -> Reach Seed Edge b.

  Definition SerializedSeed
      (ControlRoot EnvRoot KontRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    ControlRoot a \/ EnvRoot a \/ KontRoot a.

  Theorem reachable_store_in_serialized_slice :
    forall (ControlRoot EnvRoot KontRoot Slice : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, SerializedSeed ControlRoot EnvRoot KontRoot a -> Slice a) ->
      (forall a b, Slice a -> Edge a b -> Slice b) ->
      forall a,
        Reach (SerializedSeed ControlRoot EnvRoot KontRoot) Edge a ->
        Slice a.
  Proof.
    intros ControlRoot EnvRoot KontRoot Slice Edge Hseed Hclosed a Hreach.
    induction Hreach as [a Hseed_a | a b Hreach_a Hslice_a Hedge].
    - apply Hseed. exact Hseed_a.
    - apply (Hclosed a b Hslice_a Hedge).
  Qed.

  Theorem restored_future_touch_is_in_slice :
    forall (ControlRoot EnvRoot KontRoot FutureTouch Slice : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, SerializedSeed ControlRoot EnvRoot KontRoot a -> Slice a) ->
      (forall a b, Slice a -> Edge a b -> Slice b) ->
      (forall a,
          FutureTouch a ->
          Reach (SerializedSeed ControlRoot EnvRoot KontRoot) Edge a) ->
      forall a,
        FutureTouch a ->
        Slice a.
  Proof.
    intros ControlRoot EnvRoot KontRoot FutureTouch Slice Edge
           Hseed Hclosed Hfuture a Htouch.
    apply (reachable_store_in_serialized_slice
             ControlRoot EnvRoot KontRoot Slice Edge Hseed Hclosed a).
    apply Hfuture. exact Htouch.
  Qed.

  Theorem restored_future_touch_not_freed :
    forall (ControlRoot EnvRoot KontRoot FutureTouch Slice Freed :
              Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, SerializedSeed ControlRoot EnvRoot KontRoot a -> Slice a) ->
      (forall a b, Slice a -> Edge a b -> Slice b) ->
      (forall a,
          FutureTouch a ->
          Reach (SerializedSeed ControlRoot EnvRoot KontRoot) Edge a) ->
      (forall a, Freed a -> ~ Slice a) ->
      forall a,
        FutureTouch a ->
        ~ Freed a.
  Proof.
    intros ControlRoot EnvRoot KontRoot FutureTouch Slice Freed Edge
           Hseed Hclosed Hfuture Hfreed a Htouch Hfreed_a.
    apply (Hfreed a Hfreed_a).
    apply (restored_future_touch_is_in_slice
             ControlRoot EnvRoot KontRoot FutureTouch Slice Edge
             Hseed Hclosed Hfuture a Htouch).
  Qed.
End SerializableContinuationSliceModel.

End MeTTaTron_GC_SerializableContinuationSlice.
