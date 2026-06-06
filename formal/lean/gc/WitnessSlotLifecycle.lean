/-!
Lean model of the V4 witness-slot lifecycle obligation.

A mutator's witness slot stays occupied from outermost `EvalGuard::enter` to
the true outermost drop, including across safepoint drops. A sweep may proceed
only when a live machine is either still occupying its witness slot, forcing the
driver to wait, or has buffered its roots through a genuine reified park.
-/

namespace MeTTaTron.GC.WitnessSlotLifecycle

variable {Machine : Type u}

def LiveMachineVisible
    (Live Occupied Buffered : Machine -> Prop)
    (m : Machine) : Prop :=
  Live m -> Occupied m ∨ Buffered m

theorem live_machine_visible_on_sweep
    {Live Occupied Buffered Swept : Machine -> Prop}
    (visible : forall {m : Machine}, LiveMachineVisible Live Occupied Buffered m)
    (sweepGate : forall {m : Machine}, Swept m -> Not (Occupied m) ∨ Buffered m) :
    forall {m : Machine}, Swept m -> Live m -> Buffered m := by
  intro m swept live
  have rootVisible := visible live
  cases rootVisible with
  | inl occupied =>
      cases sweepGate swept with
      | inl notOccupied => exact False.elim (notOccupied occupied)
      | inr buffered => exact buffered
  | inr buffered => exact buffered

theorem safepoint_drop_preserves_live_visibility
    {Live OccupiedBefore OccupiedAfter BufferedBefore BufferedAfter : Machine -> Prop}
    (visibleBefore :
      forall {m : Machine}, LiveMachineVisible Live OccupiedBefore BufferedBefore m)
    (keepsOccupied :
      forall {m : Machine}, Live m -> OccupiedBefore m -> OccupiedAfter m)
    (keepsBuffered : forall {m : Machine}, BufferedBefore m -> BufferedAfter m) :
    forall {m : Machine},
      LiveMachineVisible Live OccupiedAfter BufferedAfter m := by
  intro m live
  cases visibleBefore live with
  | inl occupied => exact Or.inl (keepsOccupied live occupied)
  | inr buffered => exact Or.inr (keepsBuffered buffered)

end MeTTaTron.GC.WitnessSlotLifecycle
