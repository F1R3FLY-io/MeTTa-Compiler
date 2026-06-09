(** E1 default concurrent-collector flip composition.

    The lower-level E1 proofs cover individual obligations:

      - [DedicatedSingleRegime]: legacy request producers are suppressed when the
        dedicated index collector is active.
      - [ConcurrentTriggerBackstop]: a FANOUT trigger either posts a driver request
        or clears the request/resumes workers on handoff failure.
      - [RendezvousProgress]: a posted rendezvous that closes its cycle advances the
        generation and releases parked workers.
      - [DedicatedHandoff]: quiescence root vectors are never reused after a
        successful handoff.

    This file composes the default-flip safety boundary: in index mode the
    dedicated collector is the active regime; a FANOUT watermark cannot leave a
    driverless [GC_REQUESTED] cycle; and a successful driver path closes the cycle,
    advances the generation, and resumes workers. It is deliberately propositional:
    source coupling pins each premise to the shipped Rust state machine.
*)

Module MeTTaTron_GC_E1DefaultConcurrentFlip.

Section DefaultFlipModel.
  Variable LegacyProducer : Type.

  Variable IndexMode Dedicated : Prop.
  Variable FanoutWatermark : Prop.
  Variable TriggerSent TriggerFailed : Prop.
  Variable DriverPosted : Prop.
  Variable CycleClosed GenerationAdvanced : Prop.
  Variable RequestCleared WorkersResumed : Prop.
  Variable LegacyRequests : LegacyProducer -> Prop.

  Definition DefaultDedicatedFollowsIndex : Prop :=
    IndexMode -> Dedicated.

  Definition LegacyRequestsSuppressedUnderDedicated : Prop :=
    Dedicated -> forall p, ~ LegacyRequests p.

  Definition FanoutTriggerTotal : Prop :=
    Dedicated -> FanoutWatermark -> TriggerSent \/ TriggerFailed.

  Definition SuccessfulTriggerPostsDriver : Prop :=
    TriggerSent -> DriverPosted.

  Definition FailedTriggerBackstopped : Prop :=
    TriggerFailed -> RequestCleared /\ WorkersResumed.

  Definition PostedDriverClosesCycle : Prop :=
    DriverPosted -> CycleClosed.

  Definition ClosedCycleReleasesWorkers : Prop :=
    CycleClosed -> GenerationAdvanced /\ RequestCleared /\ WorkersResumed.

  Theorem default_flip_suppresses_legacy_request_producers :
    DefaultDedicatedFollowsIndex ->
    LegacyRequestsSuppressedUnderDedicated ->
    IndexMode ->
    forall p,
      ~ LegacyRequests p.
  Proof.
    intros Hdefault Hsuppressed Hindex p.
    apply Hsuppressed.
    apply Hdefault.
    exact Hindex.
  Qed.

  Theorem successful_default_trigger_releases_workers :
    SuccessfulTriggerPostsDriver ->
    PostedDriverClosesCycle ->
    ClosedCycleReleasesWorkers ->
    TriggerSent ->
    CycleClosed /\ GenerationAdvanced /\ RequestCleared /\ WorkersResumed.
  Proof.
    intros Hposts Hcloses Hreleases Hsent.
    pose proof (Hposts Hsent) as Hdriver.
    pose proof (Hcloses Hdriver) as Hclosed.
    pose proof (Hreleases Hclosed) as [Hadvanced [Hcleared Hresumed]].
    repeat split; assumption.
  Qed.

  Theorem failed_default_trigger_has_no_stuck_request :
    FailedTriggerBackstopped ->
    TriggerFailed ->
    RequestCleared /\ WorkersResumed.
  Proof.
    intros Hbackstop Hfailed.
    apply Hbackstop.
    exact Hfailed.
  Qed.

  Theorem default_flip_fanout_trigger_progress :
    DefaultDedicatedFollowsIndex ->
    FanoutTriggerTotal ->
    SuccessfulTriggerPostsDriver ->
    FailedTriggerBackstopped ->
    PostedDriverClosesCycle ->
    ClosedCycleReleasesWorkers ->
    IndexMode ->
    FanoutWatermark ->
    (CycleClosed /\ GenerationAdvanced /\ RequestCleared /\ WorkersResumed) \/
    (RequestCleared /\ WorkersResumed).
  Proof.
    intros Hdefault Htrigger Hposts Hfailed Hcloses Hreleases Hindex Hwatermark.
    pose proof (Hdefault Hindex) as Hdedicated.
    destruct (Htrigger Hdedicated Hwatermark) as [Hsent | Hfail].
    - left.
      apply (successful_default_trigger_releases_workers
               Hposts Hcloses Hreleases Hsent).
    - right.
      apply (failed_default_trigger_has_no_stuck_request Hfailed Hfail).
  Qed.

  Theorem default_flip_no_driverless_request_or_stuck_workers :
    DefaultDedicatedFollowsIndex ->
    LegacyRequestsSuppressedUnderDedicated ->
    SuccessfulTriggerPostsDriver ->
    FailedTriggerBackstopped ->
    IndexMode ->
    (exists p, LegacyRequests p) \/
      (TriggerSent /\ ~ DriverPosted) \/
      (TriggerFailed /\ ~ RequestCleared) ->
    False.
  Proof.
    intros Hdefault Hlegacy Hposts Hfailed Hindex Hbad.
    destruct Hbad as [[p Hlegacy_req] | [[Hsent Hno_driver] | [Hfail Hnot_cleared]]].
    - apply (default_flip_suppresses_legacy_request_producers
               Hdefault Hlegacy Hindex p).
      exact Hlegacy_req.
    - apply Hno_driver.
      apply Hposts.
      exact Hsent.
    - apply Hnot_cleared.
      destruct (Hfailed Hfail) as [Hcleared _].
      exact Hcleared.
  Qed.
End DefaultFlipModel.

End MeTTaTron_GC_E1DefaultConcurrentFlip.
