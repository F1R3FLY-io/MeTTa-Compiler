(** Rocq model of the E1 rendezvous witness root-union obligation. *)

Module MeTTaTron_GC_RendezvousWitness.

Section RendezvousWitnessModel.
  Variables Slot Addr : Type.

  Definition WitnessSatisfied
      (PublishedCurrent FutureAcquired : Slot -> Prop)
      (s : Slot) : Prop :=
    PublishedCurrent s \/ FutureAcquired s.

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

  Theorem non_reified_finish_keeps_current_slot_unsatisfied :
    forall (FinishedCurrent PublishedCurrent FutureAcquired : Slot -> Prop),
      (forall s, FinishedCurrent s -> ~ PublishedCurrent s) ->
      (forall s, FinishedCurrent s -> ~ FutureAcquired s) ->
      forall s,
        FinishedCurrent s ->
        ~ WitnessSatisfied PublishedCurrent FutureAcquired s.
  Proof.
    intros FinishedCurrent PublishedCurrent FutureAcquired
           Hno_publish Hnot_future s Hfinished Hsatisfied.
    destruct Hsatisfied as [Hpublished | Hfuture].
    - apply (Hno_publish s Hfinished).
      exact Hpublished.
    - apply (Hnot_future s Hfinished).
      exact Hfuture.
  Qed.

  Theorem finisher_stamp_would_satisfy_without_buffer :
    forall (FinishedCurrent PublishedCurrent FutureAcquired Buffered : Slot -> Prop),
      (forall s, PublishedCurrent s -> WitnessSatisfied PublishedCurrent FutureAcquired s) ->
      forall s,
        FinishedCurrent s ->
        PublishedCurrent s ->
        ~ Buffered s ->
        WitnessSatisfied PublishedCurrent FutureAcquired s /\ FinishedCurrent s /\ ~ Buffered s.
  Proof.
    intros FinishedCurrent PublishedCurrent FutureAcquired Buffered
           Hpublished_satisfies s Hfinished Hpublished Hnot_buffered.
    repeat split.
    - apply Hpublished_satisfies.
      exact Hpublished.
    - exact Hfinished.
    - exact Hnot_buffered.
  Qed.
End RendezvousWitnessModel.

End MeTTaTron_GC_RendezvousWitness.
