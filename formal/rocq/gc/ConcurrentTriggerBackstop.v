(** E1 FANOUT rendezvous-trigger backstop obligations.

    A FANOUT worker trigger sets [GC_REQUESTED] before handing a
    [CollectRendezvous] request to the dedicated GC thread.  If that handoff
    fails, no driver will clear the request at cycle end.  The trigger must
    therefore run the resume backstop, which clears [GC_REQUESTED] and wakes
    workers that might otherwise park forever.
*)

Module MeTTaTron_GC_ConcurrentTriggerBackstop.

Section ConcurrentTriggerBackstopModel.
  Definition TriggerFailureBackstopped
      (TriggerFailed RequestCleared WorkersResumed : Prop) : Prop :=
    TriggerFailed -> RequestCleared /\ WorkersResumed.

  Definition SuccessfulTriggerHasDriver
      (TriggerSent DriverPosted : Prop) : Prop :=
    TriggerSent -> DriverPosted.

  Theorem failed_trigger_clears_request :
    forall TriggerFailed RequestCleared WorkersResumed,
      TriggerFailureBackstopped TriggerFailed RequestCleared WorkersResumed ->
      TriggerFailed ->
      RequestCleared.
  Proof.
    intros TriggerFailed RequestCleared WorkersResumed Hbackstop Hfailed.
    destruct (Hbackstop Hfailed) as [Hcleared _].
    exact Hcleared.
  Qed.

  Theorem failed_trigger_resumes_workers :
    forall TriggerFailed RequestCleared WorkersResumed,
      TriggerFailureBackstopped TriggerFailed RequestCleared WorkersResumed ->
      TriggerFailed ->
      WorkersResumed.
  Proof.
    intros TriggerFailed RequestCleared WorkersResumed Hbackstop Hfailed.
    destruct (Hbackstop Hfailed) as [_ Hresumed].
    exact Hresumed.
  Qed.

  Theorem no_driverless_pending_request_after_trigger :
    forall TriggerSent TriggerFailed DriverPosted RequestCleared WorkersResumed,
      SuccessfulTriggerHasDriver TriggerSent DriverPosted ->
      TriggerFailureBackstopped TriggerFailed RequestCleared WorkersResumed ->
      (TriggerSent \/ TriggerFailed) ->
      ~ DriverPosted ->
      ~ RequestCleared ->
      False.
  Proof.
    intros TriggerSent TriggerFailed DriverPosted RequestCleared WorkersResumed
           Hsent_driver Hfailed_backstop Htrigger Hno_driver Hnot_cleared.
    destruct Htrigger as [Hsent | Hfailed].
    - apply Hno_driver.
      apply Hsent_driver.
      exact Hsent.
    - apply Hnot_cleared.
      apply (failed_trigger_clears_request
               TriggerFailed RequestCleared WorkersResumed Hfailed_backstop).
      exact Hfailed.
  Qed.
End ConcurrentTriggerBackstopModel.

End MeTTaTron_GC_ConcurrentTriggerBackstop.
