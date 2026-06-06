/-!
Lean model of the E1 worker-admission gate.

A dedicated collection closes admission before taking the participant/root
snapshot. Any worker live at sweep must therefore either have been present in
that snapshot or have been kept out of the active evaluator set until the
collection finished.
-/

namespace MeTTaTron.GC.WorkerAdmission

variable {Worker : Type u}

theorem admission_gate_snapshot_complete
    {JoinedAtSweep InSnapshot JoinedDuringCollection : Worker -> Prop}
    (joinedShape :
      forall {w : Worker}, JoinedAtSweep w -> InSnapshot w ∨ JoinedDuringCollection w)
    (admissionClosed : forall {w : Worker}, Not (JoinedDuringCollection w)) :
    forall {w : Worker}, JoinedAtSweep w -> InSnapshot w := by
  intro w hjoined
  cases joinedShape hjoined with
  | inl hsnapshot => exact hsnapshot
  | inr hduring => exact False.elim (admissionClosed hduring)

theorem admitted_worker_survives_sweep
    {JoinedAtSweep InSnapshot JoinedDuringCollection
      Marked Freed : Worker -> Prop}
    (joinedShape :
      forall {w : Worker}, JoinedAtSweep w -> InSnapshot w ∨ JoinedDuringCollection w)
    (admissionClosed : forall {w : Worker}, Not (JoinedDuringCollection w))
    (snapshotMarked : forall {w : Worker}, InSnapshot w -> Marked w)
    (sweepOnlyUnmarked : forall {w : Worker}, Freed w -> Not (Marked w)) :
    forall {w : Worker}, JoinedAtSweep w -> Not (Freed w) := by
  intro w hjoined hfreed
  apply sweepOnlyUnmarked hfreed
  apply snapshotMarked
  exact admission_gate_snapshot_complete
    (JoinedAtSweep := JoinedAtSweep)
    (InSnapshot := InSnapshot)
    (JoinedDuringCollection := JoinedDuringCollection)
    joinedShape
    admissionClosed
    hjoined

theorem missing_admission_gate_exposes_unsnapshotted_worker
    {JoinedAtSweep InSnapshot JoinedDuringCollection : Worker -> Prop}
    {w : Worker}
    (joined : JoinedAtSweep w)
    (during : JoinedDuringCollection w)
    (notSnapshot : Not (InSnapshot w)) :
    exists u, JoinedAtSweep u ∧ JoinedDuringCollection u ∧ Not (InSnapshot u) := by
  exact Exists.intro w (And.intro joined (And.intro during notSnapshot))

end MeTTaTron.GC.WorkerAdmission
