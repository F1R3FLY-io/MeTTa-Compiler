(** Cron startup readiness and submitted-task delivery.

    The cron manager returns two capabilities to the caller: a [CronHandle]
    containing the task sender, and a one-shot ready receiver.  The ready signal
    is useful only if it is sent from inside [CronStateMachine::run], after the
    event loop owns the task receiver.  A task submitted through the returned
    handle after observing ready must then be reachable by the event loop's
    channel polling path: either [CheckEvents] observes the task directly, or
    [DrainChannel] observes it while draining.

    This is distinct from [StructChannelPairing.v].  That proof checks endpoint
    pairing.  This one checks the cron-specific startup delivery composition.
*)

From Stdlib Require Import Bool.Bool.

Module MeTTaTron_GC_CronStartupDelivery.

Section CronStartupDeliveryModel.
  Record StartupConfig : Type := {
    handle_sender_returned : bool;
    ready_receiver_returned : bool;
    ready_sent_inside_run : bool;
    schedule_after_ready : bool;
    check_events_polls : bool;
    drain_channel_polls : bool
  }.

  Definition ready_observable (c : StartupConfig) : bool :=
    ready_receiver_returned c && ready_sent_inside_run c.

  Definition submitted_after_ready (c : StartupConfig) : bool :=
    schedule_after_ready c &&
    ready_observable c &&
    handle_sender_returned c.

  Definition event_loop_poll_path (c : StartupConfig) : bool :=
    check_events_polls c || drain_channel_polls c.

  Definition startup_delivery_safe (c : StartupConfig) : Prop :=
    submitted_after_ready c = true -> event_loop_poll_path c = true.

  Definition complete_startup : StartupConfig :=
    {| handle_sender_returned := true;
       ready_receiver_returned := true;
       ready_sent_inside_run := true;
       schedule_after_ready := true;
       check_events_polls := true;
       drain_channel_polls := true |}.

  Definition missing_ready_signal : StartupConfig :=
    {| handle_sender_returned := true;
       ready_receiver_returned := true;
       ready_sent_inside_run := false;
       schedule_after_ready := true;
       check_events_polls := true;
       drain_channel_polls := true |}.

  Definition missing_handle_sender : StartupConfig :=
    {| handle_sender_returned := false;
       ready_receiver_returned := true;
       ready_sent_inside_run := true;
       schedule_after_ready := true;
       check_events_polls := true;
       drain_channel_polls := true |}.

  Definition missing_poll_path : StartupConfig :=
    {| handle_sender_returned := true;
       ready_receiver_returned := true;
       ready_sent_inside_run := true;
       schedule_after_ready := true;
       check_events_polls := false;
       drain_channel_polls := false |}.

  Theorem complete_startup_delivers_submitted_task :
    startup_delivery_safe complete_startup.
  Proof.
    unfold startup_delivery_safe, submitted_after_ready, ready_observable,
      event_loop_poll_path, complete_startup.
    simpl.
    intros _.
    reflexivity.
  Qed.

  Theorem submitted_after_ready_requires_returned_handle :
    forall c,
      submitted_after_ready c = true ->
      handle_sender_returned c = true.
  Proof.
    intros c Hsubmitted.
    unfold submitted_after_ready in Hsubmitted.
    apply andb_true_iff in Hsubmitted as [_ Hhandle].
    exact Hhandle.
  Qed.

  Theorem submitted_after_ready_requires_ready_signal :
    forall c,
      submitted_after_ready c = true ->
      ready_receiver_returned c = true /\ ready_sent_inside_run c = true.
  Proof.
    intros c Hsubmitted.
    unfold submitted_after_ready, ready_observable in Hsubmitted.
    apply andb_true_iff in Hsubmitted as [Hschedule_ready _].
    apply andb_true_iff in Hschedule_ready as [_ Hready].
    apply andb_true_iff in Hready.
    exact Hready.
  Qed.

  Theorem missing_ready_signal_prevents_ready_observation :
    ready_observable missing_ready_signal = false.
  Proof.
    reflexivity.
  Qed.

  Theorem missing_handle_prevents_reachable_submission :
    submitted_after_ready missing_handle_sender = false.
  Proof.
    reflexivity.
  Qed.

  Theorem missing_poll_path_exposes_delivery_gap :
    ~ startup_delivery_safe missing_poll_path.
  Proof.
    unfold startup_delivery_safe, submitted_after_ready, ready_observable,
      event_loop_poll_path, missing_poll_path.
    simpl.
    intros Hsafe.
    discriminate (Hsafe eq_refl).
  Qed.
End CronStartupDeliveryModel.

End MeTTaTron_GC_CronStartupDelivery.
