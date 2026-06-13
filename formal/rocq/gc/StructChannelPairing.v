(** Struct-stored channel pairing obligations.

    This proof covers the static-analysis shape where a channel endpoint is
    stored in a struct field, cloned into a worker, or returned to a caller.
    Field-sensitive pairing is source-coupled in
    scripts/verify_cesk_gc_source_coupling.sh; this file discharges the small
    logical obligations behind those pins. *)

Module MeTTaTron_GC_StructChannelPairing.

Section ChannelPairingModel.
  Definition StoredReceiveSafe
      (ReceiverUsed PairCreated SenderStored ReceiverStored : Prop) : Prop :=
    ReceiverUsed -> PairCreated /\ SenderStored /\ ReceiverStored.

  Definition WorkerReceiveSafe
      (WorkerReceive PairCreated SenderStored WorkerReceiverCloned : Prop) : Prop :=
    WorkerReceive -> PairCreated /\ SenderStored /\ WorkerReceiverCloned.

  Definition ResponseSendSafe
      (ResponseSent ResponseSenderCloned ResponseReceiverStored : Prop) : Prop :=
    ResponseSent -> ResponseSenderCloned /\ ResponseReceiverStored.

  Definition ReadyWaitSafe
      (ReadyWait ReadySenderStored ReadyReceiverReturned ReadySignalSent : Prop) : Prop :=
    ReadyWait -> ReadySenderStored /\ ReadyReceiverReturned /\ ReadySignalSent.

  Definition PrivateWrapperDormant
      (WrapperConstructed ReceiverWait : Prop) : Prop :=
    ~ WrapperConstructed -> ~ ReceiverWait.

  Theorem stored_receive_has_sender :
    forall (ReceiverUsed PairCreated SenderStored ReceiverStored : Prop),
      (ReceiverUsed -> PairCreated) ->
      (ReceiverUsed -> SenderStored) ->
      (ReceiverUsed -> ReceiverStored) ->
      StoredReceiveSafe ReceiverUsed PairCreated SenderStored ReceiverStored.
  Proof.
    intros ReceiverUsed PairCreated SenderStored ReceiverStored
           Hpair Hsender Hreceiver Hused.
    split.
    - apply Hpair. exact Hused.
    - split.
      + apply Hsender. exact Hused.
      + apply Hreceiver. exact Hused.
  Qed.

  Theorem worker_receive_has_cloned_receiver :
    forall (WorkerReceive PairCreated SenderStored WorkerReceiverCloned : Prop),
      (WorkerReceive -> PairCreated) ->
      (WorkerReceive -> SenderStored) ->
      (WorkerReceive -> WorkerReceiverCloned) ->
      WorkerReceiveSafe WorkerReceive PairCreated SenderStored WorkerReceiverCloned.
  Proof.
    intros WorkerReceive PairCreated SenderStored WorkerReceiverCloned
           Hpair Hsender Hclone Hreceive.
    split.
    - apply Hpair. exact Hreceive.
    - split.
      + apply Hsender. exact Hreceive.
      + apply Hclone. exact Hreceive.
  Qed.

  Theorem response_send_has_caller_receiver :
    forall (ResponseSent ResponseSenderCloned ResponseReceiverStored : Prop),
      (ResponseSent -> ResponseSenderCloned) ->
      (ResponseSent -> ResponseReceiverStored) ->
      ResponseSendSafe ResponseSent ResponseSenderCloned ResponseReceiverStored.
  Proof.
    intros ResponseSent ResponseSenderCloned ResponseReceiverStored
           Hsender Hreceiver Hsent.
    split.
    - apply Hsender. exact Hsent.
    - apply Hreceiver. exact Hsent.
  Qed.

  Theorem ready_wait_has_signal_sender :
    forall (ReadyWait ReadySenderStored ReadyReceiverReturned ReadySignalSent : Prop),
      (ReadyWait -> ReadySenderStored) ->
      (ReadyWait -> ReadyReceiverReturned) ->
      (ReadyWait -> ReadySignalSent) ->
      ReadyWaitSafe ReadyWait ReadySenderStored ReadyReceiverReturned ReadySignalSent.
  Proof.
    intros ReadyWait ReadySenderStored ReadyReceiverReturned ReadySignalSent
           Hsender Hreceiver Hsignal Hwait.
    split.
    - apply Hsender. exact Hwait.
    - split.
      + apply Hreceiver. exact Hwait.
      + apply Hsignal. exact Hwait.
  Qed.

  Theorem private_wrapper_without_constructor_cannot_wait :
    forall (WrapperConstructed ReceiverWait : Prop),
      (ReceiverWait -> WrapperConstructed) ->
      PrivateWrapperDormant WrapperConstructed ReceiverWait.
  Proof.
    intros WrapperConstructed ReceiverWait Hwait_constructed Hnot_constructed Hwait.
    apply Hnot_constructed.
    apply Hwait_constructed.
    exact Hwait.
  Qed.

  Theorem struct_channel_pairing_safe :
    forall (ReceiverUsed PairCreated SenderStored ReceiverStored
           WorkerReceive WorkerReceiverCloned
           ResponseSent ResponseSenderCloned ResponseReceiverStored
           ReadyWait ReadySenderStored ReadyReceiverReturned ReadySignalSent : Prop),
      (ReceiverUsed -> PairCreated) ->
      (ReceiverUsed -> SenderStored) ->
      (ReceiverUsed -> ReceiverStored) ->
      (WorkerReceive -> PairCreated) ->
      (WorkerReceive -> SenderStored) ->
      (WorkerReceive -> WorkerReceiverCloned) ->
      (ResponseSent -> ResponseSenderCloned) ->
      (ResponseSent -> ResponseReceiverStored) ->
      (ReadyWait -> ReadySenderStored) ->
      (ReadyWait -> ReadyReceiverReturned) ->
      (ReadyWait -> ReadySignalSent) ->
      ReceiverUsed ->
      WorkerReceive ->
      ResponseSent ->
      ReadyWait ->
      (PairCreated /\ SenderStored /\ ReceiverStored) /\
      (PairCreated /\ SenderStored /\ WorkerReceiverCloned) /\
      (ResponseSenderCloned /\ ResponseReceiverStored) /\
      (ReadySenderStored /\ ReadyReceiverReturned /\ ReadySignalSent).
  Proof.
    intros ReceiverUsed PairCreated SenderStored ReceiverStored
           WorkerReceive WorkerReceiverCloned
           ResponseSent ResponseSenderCloned ResponseReceiverStored
           ReadyWait ReadySenderStored ReadyReceiverReturned ReadySignalSent
           Hrecv_pair Hrecv_sender Hrecv_receiver
           Hworker_pair Hworker_sender Hworker_clone
           Hresp_sender Hresp_receiver
           Hready_sender Hready_receiver Hready_signal
           Hrecv Hworker Hresp Hready.
    split.
    - apply (stored_receive_has_sender
        ReceiverUsed PairCreated SenderStored ReceiverStored);
        assumption.
    - split.
      + apply (worker_receive_has_cloned_receiver
          WorkerReceive PairCreated SenderStored WorkerReceiverCloned);
          assumption.
      + split.
        * apply (response_send_has_caller_receiver
            ResponseSent ResponseSenderCloned ResponseReceiverStored);
            assumption.
        * apply (ready_wait_has_signal_sender
            ReadyWait ReadySenderStored ReadyReceiverReturned ReadySignalSent);
            assumption.
  Qed.
End ChannelPairingModel.

End MeTTaTron_GC_StructChannelPairing.
