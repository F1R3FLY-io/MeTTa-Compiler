(** Rocq model of the E1 worker-admission gate.

    A dedicated collection closes admission before taking the participant/root
    snapshot.  Any worker that is live at sweep must therefore either have been
    present in that snapshot or have been kept out of the active evaluator set
    until the collection finished.  The companion TLA+ model discriminates the
    race where a worker enters during collection after the snapshot.
*)

Module MeTTaTron_GC_WorkerAdmission.

Section WorkerAdmissionModel.
  Variable Worker : Type.

  Theorem admission_gate_snapshot_complete :
    forall (JoinedAtSweep InSnapshot JoinedDuringCollection : Worker -> Prop),
      (forall w, JoinedAtSweep w -> InSnapshot w \/ JoinedDuringCollection w) ->
      (forall w, ~ JoinedDuringCollection w) ->
      forall w,
        JoinedAtSweep w -> InSnapshot w.
  Proof.
    intros JoinedAtSweep InSnapshot JoinedDuringCollection Hjoined_shape Hadmission_closed
           w Hjoined.
    destruct (Hjoined_shape w Hjoined) as [Hsnapshot | Hduring].
    - exact Hsnapshot.
    - exfalso.
      apply (Hadmission_closed w).
      exact Hduring.
  Qed.

  Theorem admitted_worker_survives_sweep :
    forall (JoinedAtSweep InSnapshot JoinedDuringCollection
            Marked Freed : Worker -> Prop),
      (forall w, JoinedAtSweep w -> InSnapshot w \/ JoinedDuringCollection w) ->
      (forall w, ~ JoinedDuringCollection w) ->
      (forall w, InSnapshot w -> Marked w) ->
      (forall w, Freed w -> ~ Marked w) ->
      forall w,
        JoinedAtSweep w -> ~ Freed w.
  Proof.
    intros JoinedAtSweep InSnapshot JoinedDuringCollection Marked Freed
           Hjoined_shape Hadmission_closed Hsnapshot_marked Hsweep w Hjoined Hfreed.
    apply (Hsweep w Hfreed).
    apply Hsnapshot_marked.
    apply (admission_gate_snapshot_complete
             JoinedAtSweep InSnapshot JoinedDuringCollection).
    - exact Hjoined_shape.
    - exact Hadmission_closed.
    - exact Hjoined.
  Qed.

  Theorem missing_admission_gate_exposes_unsnapshotted_worker :
    forall (JoinedAtSweep InSnapshot JoinedDuringCollection : Worker -> Prop)
           (w : Worker),
      JoinedAtSweep w ->
      JoinedDuringCollection w ->
      ~ InSnapshot w ->
      exists u,
        JoinedAtSweep u /\ JoinedDuringCollection u /\ ~ InSnapshot u.
  Proof.
    intros JoinedAtSweep InSnapshot JoinedDuringCollection w Hjoined Hduring Hnot_snapshot.
    exists w.
    split.
    - exact Hjoined.
    - split.
      + exact Hduring.
      + exact Hnot_snapshot.
  Qed.
End WorkerAdmissionModel.

End MeTTaTron_GC_WorkerAdmission.
