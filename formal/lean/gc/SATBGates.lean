/-!
Lean model of the E2 SATB phase and sweep gates.

The deletion barrier alone is not enough unless marker start and final sweep are
ordered against in-flight deletions. TLC checks the finite discriminators in
`SATBPhaseGate.tla` and `SATBSweepGate.tla`; these theorems prove the
source-level coverage premise those models represent.
-/

namespace MeTTaTron.GC.SATBGates

variable {Addr : Type u}

theorem phase_gate_removed_snapshot_preimage_shaded
    {InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
      AfterStartDeletion RemovedBySweep Shaded SnapshotLive : Addr -> Prop}
    (snapshotVisible :
      forall {a : Addr}, SnapshotLive a -> InCacheAtStart a)
    (preStartState :
      forall {a : Addr}, BeganBeforeStart a -> CommittedBeforeStart a ∨ OpenAtStart a)
    (committedInvisible :
      forall {a : Addr}, CommittedBeforeStart a -> Not (InCacheAtStart a))
    (phaseGateClosed : forall {a : Addr}, Not (OpenAtStart a))
    (removedShape :
      forall {a : Addr}, RemovedBySweep a -> BeganBeforeStart a ∨ AfterStartDeletion a)
    (afterStartShaded :
      forall {a : Addr}, AfterStartDeletion a -> Shaded a) :
    forall {a : Addr}, SnapshotLive a -> RemovedBySweep a -> Shaded a := by
  intro a hsnapshot hremoved
  cases removedShape hremoved with
  | inl hbefore =>
      cases preStartState hbefore with
      | inl hcommitted =>
          exact False.elim (committedInvisible hcommitted (snapshotVisible hsnapshot))
      | inr hopen =>
          exact False.elim (phaseGateClosed hopen)
  | inr hafter =>
      exact afterStartShaded hafter

theorem phase_gate_snapshot_live_survives_sweep
    {InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
      AfterStartDeletion RemovedBySweep InCacheAtSweep Shaded
      SnapshotLive Marked Freed : Addr -> Prop}
    (snapshotVisible :
      forall {a : Addr}, SnapshotLive a -> InCacheAtStart a)
    (preStartState :
      forall {a : Addr}, BeganBeforeStart a -> CommittedBeforeStart a ∨ OpenAtStart a)
    (committedInvisible :
      forall {a : Addr}, CommittedBeforeStart a -> Not (InCacheAtStart a))
    (phaseGateClosed : forall {a : Addr}, Not (OpenAtStart a))
    (removedShape :
      forall {a : Addr}, RemovedBySweep a -> BeganBeforeStart a ∨ AfterStartDeletion a)
    (afterStartShaded :
      forall {a : Addr}, AfterStartDeletion a -> Shaded a)
    (snapshotShape :
      forall {a : Addr}, SnapshotLive a -> RemovedBySweep a ∨ InCacheAtSweep a)
    (visibleMarked : forall {a : Addr}, InCacheAtSweep a -> Marked a)
    (shadedMarked : forall {a : Addr}, Shaded a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, SnapshotLive a -> Not (Freed a) := by
  intro a hsnapshot hfreed
  apply sweepOnlyUnmarked hfreed
  cases snapshotShape hsnapshot with
  | inl hremoved =>
      apply shadedMarked
      exact phase_gate_removed_snapshot_preimage_shaded
        (InCacheAtStart := InCacheAtStart)
        (BeganBeforeStart := BeganBeforeStart)
        (CommittedBeforeStart := CommittedBeforeStart)
        (OpenAtStart := OpenAtStart)
        (AfterStartDeletion := AfterStartDeletion)
        (RemovedBySweep := RemovedBySweep)
        (Shaded := Shaded)
        (SnapshotLive := SnapshotLive)
        snapshotVisible
        preStartState
        committedInvisible
        phaseGateClosed
        removedShape
        afterStartShaded
        hsnapshot
        hremoved
  | inr hvisible =>
      exact visibleMarked hvisible

theorem sweep_gate_visible_or_shaded_at_sweep
    {Sweep : Prop}
    {DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
      InCacheAtSweep Shaded SnapshotLive : Addr -> Prop}
    (sweepGate : Sweep -> forall {a : Addr}, Not (DeleteOpenAtSweep a))
    (removedState :
      forall {a : Addr}, RemovedBeforeSweep a ->
        DeleteOpenAtSweep a ∨ CommittedBeforeSweep a)
    (committedShaded :
      forall {a : Addr}, CommittedBeforeSweep a -> Shaded a)
    (snapshotShape :
      forall {a : Addr}, SnapshotLive a -> RemovedBeforeSweep a ∨ InCacheAtSweep a) :
    Sweep -> forall {a : Addr}, SnapshotLive a -> InCacheAtSweep a ∨ Shaded a := by
  intro hsweep a hsnapshot
  cases snapshotShape hsnapshot with
  | inl hremoved =>
      cases removedState hremoved with
      | inl hopen =>
          exact False.elim (sweepGate hsweep hopen)
      | inr hcommitted =>
          exact Or.inr (committedShaded hcommitted)
  | inr hvisible =>
      exact Or.inl hvisible

theorem sweep_gate_snapshot_live_survives_sweep
    {Sweep : Prop}
    {DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
      InCacheAtSweep Shaded SnapshotLive Marked Freed : Addr -> Prop}
    (sweepGate : Sweep -> forall {a : Addr}, Not (DeleteOpenAtSweep a))
    (removedState :
      forall {a : Addr}, RemovedBeforeSweep a ->
        DeleteOpenAtSweep a ∨ CommittedBeforeSweep a)
    (committedShaded :
      forall {a : Addr}, CommittedBeforeSweep a -> Shaded a)
    (snapshotShape :
      forall {a : Addr}, SnapshotLive a -> RemovedBeforeSweep a ∨ InCacheAtSweep a)
    (visibleMarked : forall {a : Addr}, InCacheAtSweep a -> Marked a)
    (shadedMarked : forall {a : Addr}, Shaded a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    Sweep -> forall {a : Addr}, SnapshotLive a -> Not (Freed a) := by
  intro hsweep a hsnapshot hfreed
  apply sweepOnlyUnmarked hfreed
  have visibleOrShaded :
      InCacheAtSweep a ∨ Shaded a :=
    sweep_gate_visible_or_shaded_at_sweep
      (Sweep := Sweep)
      (DeleteOpenAtSweep := DeleteOpenAtSweep)
      (RemovedBeforeSweep := RemovedBeforeSweep)
      (CommittedBeforeSweep := CommittedBeforeSweep)
      (InCacheAtSweep := InCacheAtSweep)
      (Shaded := Shaded)
      (SnapshotLive := SnapshotLive)
      sweepGate
      removedState
      committedShaded
      snapshotShape
      hsweep
      hsnapshot
  cases visibleOrShaded with
  | inl hvisible => exact visibleMarked hvisible
  | inr hshaded => exact shadedMarked hshaded

end MeTTaTron.GC.SATBGates
