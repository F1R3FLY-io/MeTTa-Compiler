(** E1 dedicated-GC single-regime obligations.

    When the dedicated index-GC collector is enabled, legacy cooperative
    producers must not create driverless `GC_REQUESTED` cycles.  The only valid
    request producer in that regime is the dedicated driver path, which both
    sets the request and posts the rendezvous work item.
*)

Module MeTTaTron_GC_DedicatedSingleRegime.

Section DedicatedSingleRegimeModel.
  Inductive LegacyProducer : Type :=
  | DefaultSafepoint : LegacyProducer
  | SessionSafepoint : LegacyProducer
  | ParallelSafepoint : LegacyProducer
  | CronAsync : LegacyProducer.

  Definition LegacyProducerSuppressed
      (Dedicated : Prop)
      (LegacyRequests : LegacyProducer -> Prop) : Prop :=
    Dedicated -> forall p, ~ LegacyRequests p.

  Definition DedicatedRequestSafe
      (DedicatedRequest DriverPosted : Prop) : Prop :=
    DedicatedRequest -> DriverPosted.

  Theorem dedicated_enabled_blocks_legacy_requests :
    forall Dedicated LegacyRequests,
      LegacyProducerSuppressed Dedicated LegacyRequests ->
      Dedicated ->
      forall p, ~ LegacyRequests p.
  Proof.
    intros Dedicated LegacyRequests Hsuppressed Hdedicated p.
    apply Hsuppressed.
    exact Hdedicated.
  Qed.

  Theorem dedicated_request_has_driver :
    forall DedicatedRequest DriverPosted,
      DedicatedRequestSafe DedicatedRequest DriverPosted ->
      DedicatedRequest ->
      DriverPosted.
  Proof.
    intros DedicatedRequest DriverPosted Hsafe Hrequest.
    apply Hsafe.
    exact Hrequest.
  Qed.

  Theorem no_driverless_request_under_dedicated :
    forall Dedicated LegacyRequests DedicatedRequest DriverPosted,
      LegacyProducerSuppressed Dedicated LegacyRequests ->
      DedicatedRequestSafe DedicatedRequest DriverPosted ->
      Dedicated ->
      (DedicatedRequest \/ exists p, LegacyRequests p) ->
      ~ DriverPosted ->
      False.
  Proof.
    intros Dedicated LegacyRequests DedicatedRequest DriverPosted
           Hlegacy_suppressed Hdedicated_safe Hdedicated Hrequest Hno_driver.
    destruct Hrequest as [Hdedicated_request | [p Hlegacy_request]].
    - apply Hno_driver.
      apply Hdedicated_safe.
      exact Hdedicated_request.
    - pose proof (Hlegacy_suppressed Hdedicated p) as Hno_legacy.
      apply Hno_legacy.
      exact Hlegacy_request.
  Qed.
End DedicatedSingleRegimeModel.

End MeTTaTron_GC_DedicatedSingleRegime.
