/-!
E2 batch-result handoff rooting obligations.

`run_state_async` receives result vectors from worker tasks after those workers
have left the rendezvous participant set. The values must therefore be rooted by
a persistent driver-C/safepoint handle until the caller copies them into
`MettaState.output`, where they become driver program roots. This proof captures
that handoff shape independently of the concrete Rust container used to
transport the values.
-/

namespace MeTTaTron.GC.BatchHandoff

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def CollectorRoot
    (StructuralRoot DriverRoot : Addr -> Prop)
    (a : Addr) : Prop :=
  StructuralRoot a ∨ DriverRoot a

def BatchHandoffRoot
    (HandleRoot OutputRoot : Addr -> Prop)
    (a : Addr) : Prop :=
  HandleRoot a ∨ OutputRoot a

theorem batch_result_handoff_rooted
    {BatchResult HandleRoot OutputRoot : Addr -> Prop}
    (covered : forall {a : Addr}, BatchResult a -> HandleRoot a ∨ OutputRoot a) :
    forall {a : Addr},
      BatchResult a ->
      BatchHandoffRoot HandleRoot OutputRoot a := by
  intro a hresult
  exact covered hresult

theorem batch_result_survives_while_handle_live
    {StructuralRoot DriverRoot HandleRoot : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (handleDriver : forall {a : Addr}, HandleRoot a -> DriverRoot a)
    (markComplete :
      forall {a : Addr}, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, HandleRoot a -> Not (Freed a) := by
  intro a hhandle hfreed
  have hroot : CollectorRoot StructuralRoot DriverRoot a :=
    Or.inr (handleDriver hhandle)
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

theorem batch_result_survives_after_output_copy
    {StructuralRoot DriverRoot OutputRoot : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (outputDriver : forall {a : Addr}, OutputRoot a -> DriverRoot a)
    (markComplete :
      forall {a : Addr}, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, OutputRoot a -> Not (Freed a) := by
  intro a houtput hfreed
  have hroot : CollectorRoot StructuralRoot DriverRoot a :=
    Or.inr (outputDriver houtput)
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

theorem batch_result_handoff_survives_collection
    {StructuralRoot DriverRoot BatchResult HandleRoot OutputRoot : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (covered : forall {a : Addr}, BatchResult a -> HandleRoot a ∨ OutputRoot a)
    (handleDriver : forall {a : Addr}, HandleRoot a -> DriverRoot a)
    (outputDriver : forall {a : Addr}, OutputRoot a -> DriverRoot a)
    (markComplete :
      forall {a : Addr}, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, BatchResult a -> Not (Freed a) := by
  intro a hresult hfreed
  cases covered hresult with
  | inl hhandle =>
      have hroot : CollectorRoot StructuralRoot DriverRoot a :=
        Or.inr (handleDriver hhandle)
      have hmarked := markComplete (Reach.root hroot)
      exact sweepOnlyUnmarked hfreed hmarked
  | inr houtput =>
      have hroot : CollectorRoot StructuralRoot DriverRoot a :=
        Or.inr (outputDriver houtput)
      have hmarked := markComplete (Reach.root hroot)
      exact sweepOnlyUnmarked hfreed hmarked

theorem dropping_handle_after_output_copy_is_safe
    {StructuralRoot DriverRoot BatchResult OutputRoot : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (copied : forall {a : Addr}, BatchResult a -> OutputRoot a)
    (outputDriver : forall {a : Addr}, OutputRoot a -> DriverRoot a)
    (markComplete :
      forall {a : Addr}, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, BatchResult a -> Not (Freed a) := by
  intro a hresult hfreed
  have hroot : CollectorRoot StructuralRoot DriverRoot a :=
    Or.inr (outputDriver (copied hresult))
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.BatchHandoff
