(** Rocq model of the E1 rendezvous witness root-union obligation. *)

Module MeTTaTron_GC_RendezvousWitness.

Section RendezvousWitnessModel.
  Variables Slot Addr : Type.

  Theorem witness_wait_union_complete :
    forall (Occupied Published : Slot -> Prop)
           (SlotRoot : Slot -> Addr -> Prop)
           (BufferRoot DriverRoot : Addr -> Prop),
      (forall s, Occupied s -> Published s) ->
      (forall s a, Published s -> SlotRoot s a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      forall s a, Occupied s -> SlotRoot s a -> DriverRoot a.
  Proof.
    intros Occupied Published SlotRoot BufferRoot DriverRoot
           Hwait Hbuffer Hdrain s a Hoccupied Hroot.
    apply Hdrain.
    apply (Hbuffer s a).
    - apply Hwait.
      exact Hoccupied.
    - exact Hroot.
  Qed.

  Theorem witness_roots_retain_after_mark :
    forall (Occupied Published : Slot -> Prop)
           (SlotRoot : Slot -> Addr -> Prop)
           (BufferRoot DriverRoot Marked Freed : Addr -> Prop),
      (forall s, Occupied s -> Published s) ->
      (forall s a, Published s -> SlotRoot s a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall s a, Occupied s -> SlotRoot s a -> ~ Freed a.
  Proof.
    intros Occupied Published SlotRoot BufferRoot DriverRoot Marked Freed
           Hwait Hbuffer Hdrain Hmark Hsweep s a Hoccupied Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    eapply witness_wait_union_complete; eauto.
  Qed.
End RendezvousWitnessModel.

End MeTTaTron_GC_RendezvousWitness.
