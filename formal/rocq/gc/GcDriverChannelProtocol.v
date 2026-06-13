(** Dedicated GC driver channel-protocol obligations.

    This file isolates the producer/consumer facts behind the dedicated
    `gc_driver` request loop:

    - the request receiver used by the driver is the receiver paired with the
      stored request sender created at spawn;
    - a synchronous `Collect` request carries its own response sender and the
      caller owns the paired response receiver before waiting;
    - the driver attempts a response for every handled `Collect` request;
    - fire-and-forget requests (`CollectRendezvous` and `Shutdown`) create no
      response wait.

    The separate `DedicatedHandoff` proof owns the root-vector transfer rule
    after a successful send.  This protocol proof owns the channel-liveness
    shape reported by the pgmcp channel audit.
*)

Module MeTTaTron_GC_GcDriverChannelProtocol.

Section ChannelProtocolModel.
  Definition RequestReceiveSafe
      (DriverReceived RequestSenderStored RequestReceiverOwned : Prop) : Prop :=
    DriverReceived -> RequestSenderStored /\ RequestReceiverOwned.

  Definition ResponseWaitSafe
      (CallerWaiting ResponseSenderCarried ResponseReceiverOwned ReplyAttempted : Prop)
      : Prop :=
    CallerWaiting -> ResponseSenderCarried /\ ResponseReceiverOwned /\ ReplyAttempted.

  Definition ReplySendNotOrphaned
      (ReplyAttempted ResponseSenderCarried ResponseReceiverOwned CallerWaiting : Prop)
      : Prop :=
    ReplyAttempted -> ResponseSenderCarried /\ ResponseReceiverOwned /\ CallerWaiting.

  Definition FireAndForgetSafe (RequestSent CallerWaiting ReplyAttempted : Prop) : Prop :=
    RequestSent -> ~ CallerWaiting /\ ~ ReplyAttempted.

  Theorem spawned_request_receive_has_producer :
    forall (DriverReceived RequestSenderStored RequestReceiverOwned : Prop),
      (DriverReceived -> RequestReceiverOwned) ->
      (RequestReceiverOwned -> RequestSenderStored) ->
      RequestReceiveSafe DriverReceived RequestSenderStored RequestReceiverOwned.
  Proof.
    intros DriverReceived RequestSenderStored RequestReceiverOwned
           Hreceived_receiver Hreceiver_sender Hreceived.
    split.
    - apply Hreceiver_sender.
      apply Hreceived_receiver.
      exact Hreceived.
    - apply Hreceived_receiver.
      exact Hreceived.
  Qed.

  Theorem successful_collect_wait_has_response_producer :
    forall (CollectSent ResponseSenderCarried ResponseReceiverOwned DriverHandled
           ReplyAttempted CallerWaiting : Prop),
      (CallerWaiting -> CollectSent) ->
      (CollectSent -> ResponseSenderCarried) ->
      (CollectSent -> ResponseReceiverOwned) ->
      (CollectSent -> DriverHandled) ->
      (DriverHandled -> ReplyAttempted) ->
      ResponseWaitSafe
        CallerWaiting ResponseSenderCarried ResponseReceiverOwned ReplyAttempted.
  Proof.
    intros CollectSent ResponseSenderCarried ResponseReceiverOwned DriverHandled
           ReplyAttempted CallerWaiting
           Hwait_sent Hsent_sender Hsent_receiver Hsent_handled Hhandled_reply Hwaiting.
    split.
    - apply Hsent_sender.
      apply Hwait_sent.
      exact Hwaiting.
    - split.
      + apply Hsent_receiver.
        apply Hwait_sent.
        exact Hwaiting.
      + apply Hhandled_reply.
        apply Hsent_handled.
        apply Hwait_sent.
        exact Hwaiting.
  Qed.

  Theorem reply_attempt_is_not_orphaned :
    forall (ReplyAttempted DriverHandled CollectSent ResponseSenderCarried
           ResponseReceiverOwned CallerWaiting : Prop),
      (ReplyAttempted -> DriverHandled) ->
      (DriverHandled -> CollectSent) ->
      (CollectSent -> ResponseSenderCarried) ->
      (CollectSent -> ResponseReceiverOwned) ->
      (ReplyAttempted -> CallerWaiting) ->
      ReplySendNotOrphaned
        ReplyAttempted ResponseSenderCarried ResponseReceiverOwned CallerWaiting.
  Proof.
    intros ReplyAttempted DriverHandled CollectSent ResponseSenderCarried
           ResponseReceiverOwned CallerWaiting
           Hreply_handled Hhandled_sent Hsent_sender Hsent_receiver Hreply_wait Hreply.
    split.
    - apply Hsent_sender.
      apply Hhandled_sent.
      apply Hreply_handled.
      exact Hreply.
    - split.
      + apply Hsent_receiver.
        apply Hhandled_sent.
        apply Hreply_handled.
        exact Hreply.
      + apply Hreply_wait.
        exact Hreply.
  Qed.

  Theorem fire_and_forget_request_has_no_response_wait :
    forall (RequestSent CallerWaiting ReplyAttempted : Prop),
      (RequestSent -> ~ CallerWaiting) ->
      (RequestSent -> ~ ReplyAttempted) ->
      FireAndForgetSafe RequestSent CallerWaiting ReplyAttempted.
  Proof.
    intros RequestSent CallerWaiting ReplyAttempted Hno_wait Hno_reply Hsent.
    split.
    - apply Hno_wait.
      exact Hsent.
    - apply Hno_reply.
      exact Hsent.
  Qed.

  Theorem gc_driver_channel_protocol_safe :
    forall (RequestSenderStored RequestReceiverOwned CollectSent
           ResponseSenderCarried ResponseReceiverOwned DriverReceivedCollect
           ReplyAttempted CallerWaiting : Prop),
      (DriverReceivedCollect -> RequestReceiverOwned) ->
      (RequestReceiverOwned -> RequestSenderStored) ->
      (CallerWaiting -> CollectSent) ->
      (CollectSent -> ResponseSenderCarried) ->
      (CollectSent -> ResponseReceiverOwned) ->
      (CollectSent -> DriverReceivedCollect) ->
      (DriverReceivedCollect -> ReplyAttempted) ->
      CallerWaiting ->
      (RequestSenderStored /\ RequestReceiverOwned) /\
      (ResponseSenderCarried /\ ResponseReceiverOwned /\ ReplyAttempted).
  Proof.
    intros RequestSenderStored RequestReceiverOwned CollectSent
           ResponseSenderCarried ResponseReceiverOwned DriverReceivedCollect
           ReplyAttempted CallerWaiting
           Hreceived_receiver Hreceiver_sender Hwait_sent Hsent_sender Hsent_receiver
           Hsent_received Hreceived_reply Hwaiting.
    split.
    - apply (spawned_request_receive_has_producer
        DriverReceivedCollect RequestSenderStored RequestReceiverOwned).
      + exact Hreceived_receiver.
      + exact Hreceiver_sender.
      + apply Hsent_received.
        apply Hwait_sent.
        exact Hwaiting.
    - apply (successful_collect_wait_has_response_producer
        CollectSent ResponseSenderCarried ResponseReceiverOwned DriverReceivedCollect
        ReplyAttempted CallerWaiting).
      + exact Hwait_sent.
      + exact Hsent_sender.
      + exact Hsent_receiver.
      + exact Hsent_received.
      + exact Hreceived_reply.
      + exact Hwaiting.
  Qed.
End ChannelProtocolModel.

End MeTTaTron_GC_GcDriverChannelProtocol.
