(** Rocq model of the scheduler/thread-pool boundary for the CESK GC.

    This module deliberately proves only the GC-facing contract, not general
    scheduler fairness or work-stealing correctness. At sweep, every address
    held alive by scheduler/thread-pool machinery must be covered by one of the
    driver root channels, or it must be impossible because admission is closed:

      - active workers publish their structural CESK roots;
      - live dispatch/collapse fan-outs are live-dispatch anchors;
      - async batch handoff values are safepoint/driver-C roots;
      - workers newly admitted during the closed collection window do not exist.

    Under those premises, ordinary mark/sweep cannot free a scheduler-held live
    address.
*)

Module MeTTaTron_GC_SchedulerGcBoundary.

Section SchedulerGcBoundaryModel.
  Variable Addr : Type.

  Definition EmptyRoot : Addr -> Prop := fun _ => False.

  Definition SchedulerLiveRoot
      (ActiveWorkerRoot DispatchFanoutRoot BatchHandoffRoot NewlyAdmittedRoot
         : Addr -> Prop)
      (a : Addr) : Prop :=
    ActiveWorkerRoot a \/
    DispatchFanoutRoot a \/
    BatchHandoffRoot a \/
    NewlyAdmittedRoot a.

  Theorem scheduler_live_root_in_driver_root :
    forall (ActiveWorkerRoot DispatchFanoutRoot BatchHandoffRoot NewlyAdmittedRoot
            DriverRoot : Addr -> Prop),
      (forall a, ActiveWorkerRoot a -> DriverRoot a) ->
      (forall a, DispatchFanoutRoot a -> DriverRoot a) ->
      (forall a, BatchHandoffRoot a -> DriverRoot a) ->
      (forall a, NewlyAdmittedRoot a -> False) ->
      forall a,
        SchedulerLiveRoot ActiveWorkerRoot DispatchFanoutRoot BatchHandoffRoot
                          NewlyAdmittedRoot a ->
        DriverRoot a.
  Proof.
    intros ActiveWorkerRoot DispatchFanoutRoot BatchHandoffRoot NewlyAdmittedRoot
           DriverRoot Hworker Hdispatch Hbatch Hadmission_closed a Hlive.
    destruct Hlive as [Hworker_a | [Hdispatch_a | [Hbatch_a | Hnew_a]]].
    - apply Hworker. exact Hworker_a.
    - apply Hdispatch. exact Hdispatch_a.
    - apply Hbatch. exact Hbatch_a.
    - exfalso.
      apply (Hadmission_closed a). exact Hnew_a.
  Qed.

  Theorem scheduler_live_root_survives_collection :
    forall (ActiveWorkerRoot DispatchFanoutRoot BatchHandoffRoot NewlyAdmittedRoot
            DriverRoot Marked Freed : Addr -> Prop),
      (forall a, ActiveWorkerRoot a -> DriverRoot a) ->
      (forall a, DispatchFanoutRoot a -> DriverRoot a) ->
      (forall a, BatchHandoffRoot a -> DriverRoot a) ->
      (forall a, NewlyAdmittedRoot a -> False) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        SchedulerLiveRoot ActiveWorkerRoot DispatchFanoutRoot BatchHandoffRoot
                          NewlyAdmittedRoot a ->
        ~ Freed a.
  Proof.
    intros ActiveWorkerRoot DispatchFanoutRoot BatchHandoffRoot NewlyAdmittedRoot
           DriverRoot Marked Freed Hworker Hdispatch Hbatch Hadmission_closed
           Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply (scheduler_live_root_in_driver_root
             ActiveWorkerRoot DispatchFanoutRoot BatchHandoffRoot
             NewlyAdmittedRoot DriverRoot).
    - exact Hworker.
    - exact Hdispatch.
    - exact Hbatch.
    - exact Hadmission_closed.
    - exact Hlive.
  Qed.

  Theorem closed_admission_rejects_newly_admitted_root :
    forall (NewlyAdmittedRoot : Addr -> Prop),
      (exists a, NewlyAdmittedRoot a) ->
      ~ (forall a, NewlyAdmittedRoot a -> False).
  Proof.
    intros NewlyAdmittedRoot [a Hnew] Hadmission_closed.
    apply (Hadmission_closed a).
    exact Hnew.
  Qed.

  Theorem uncovered_newly_admitted_root_exposes_boundary_gap :
    forall (NewlyAdmittedRoot DriverRoot : Addr -> Prop),
      (exists a, NewlyAdmittedRoot a /\ ~ DriverRoot a) ->
      exists a,
        SchedulerLiveRoot EmptyRoot EmptyRoot EmptyRoot NewlyAdmittedRoot a /\
        ~ DriverRoot a.
  Proof.
    intros NewlyAdmittedRoot DriverRoot [a [Hnew Hnot_driver]].
    exists a.
    split.
    - right. right. right. exact Hnew.
    - exact Hnot_driver.
  Qed.
End SchedulerGcBoundaryModel.

End MeTTaTron_GC_SchedulerGcBoundary.
