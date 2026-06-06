(** Rocq model of the V4 witness-slot lifecycle obligation.

    A mutator's witness slot must stay occupied from the outermost EvalGuard
    enter until the true outermost drop, including across safepoint drops.  A
    collection sweep is allowed only when a live machine is either still
    occupying its witness slot (so the driver waits) or has buffered its roots
    through a genuine reified park.
*)

Module MeTTaTron_GC_WitnessSlotLifecycle.

Section WitnessSlotLifecycleModel.
  Variable Machine : Type.

  Definition LiveMachineVisible
      (Live Occupied Buffered : Machine -> Prop)
      (m : Machine) : Prop :=
    Live m -> Occupied m \/ Buffered m.

  Theorem live_machine_visible_on_sweep :
    forall (Live Occupied Buffered Swept : Machine -> Prop),
      (forall m, LiveMachineVisible Live Occupied Buffered m) ->
      (forall m, Swept m -> ~ Occupied m \/ Buffered m) ->
      forall m,
        Swept m -> Live m -> Buffered m.
  Proof.
    intros Live Occupied Buffered Swept Hvisible Hgate m Hswept Hlive.
    destruct (Hvisible m Hlive) as [Hoccupied | Hbuffered].
    - destruct (Hgate m Hswept) as [Hnot_occupied | Hbuffered].
      + exfalso. apply Hnot_occupied. exact Hoccupied.
      + exact Hbuffered.
    - exact Hbuffered.
  Qed.

  Theorem safepoint_drop_preserves_live_visibility :
    forall (Live OccupiedBefore OccupiedAfter BufferedBefore BufferedAfter : Machine -> Prop),
      (forall m, LiveMachineVisible Live OccupiedBefore BufferedBefore m) ->
      (forall m, Live m -> OccupiedBefore m -> OccupiedAfter m) ->
      (forall m, BufferedBefore m -> BufferedAfter m) ->
      forall m,
        LiveMachineVisible Live OccupiedAfter BufferedAfter m.
  Proof.
    intros Live OccupiedBefore OccupiedAfter BufferedBefore BufferedAfter
           Hvisible Hkeeps_occupied Hkeeps_buffered m Hlive.
    destruct (Hvisible m Hlive) as [Hoccupied | Hbuffered].
    - left. apply Hkeeps_occupied; exact Hoccupied || exact Hlive.
    - right. apply Hkeeps_buffered. exact Hbuffered.
  Qed.
End WitnessSlotLifecycleModel.

End MeTTaTron_GC_WitnessSlotLifecycle.
