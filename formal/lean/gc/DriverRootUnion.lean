/-!
Lean model of the E1/E2 driver root-union channel obligation.

The dedicated collector builds the driver root set from explicit channels:
worker self-publications, safepoint roots, live environment anchors, and live
dispatch anchors. These theorems state the safety shape checked by the TLA+
discriminator: if every channel is included in the driver root set, mark/sweep
cannot free any live channel root.
-/

namespace MeTTaTron.GC.DriverRootUnion

variable {Addr : Type u}

def DriverRootUnion
    (WorkerRoot SafepointRoot EnvAnchor DispatchAnchor : Addr -> Prop)
    (a : Addr) : Prop :=
  WorkerRoot a ∨ SafepointRoot a ∨ EnvAnchor a ∨ DispatchAnchor a

theorem driver_root_union_complete
    {WorkerRoot SafepointRoot EnvAnchor DispatchAnchor DriverRoot : Addr -> Prop}
    (workerIncluded : forall {a : Addr}, WorkerRoot a -> DriverRoot a)
    (safepointIncluded : forall {a : Addr}, SafepointRoot a -> DriverRoot a)
    (envIncluded : forall {a : Addr}, EnvAnchor a -> DriverRoot a)
    (dispatchIncluded : forall {a : Addr}, DispatchAnchor a -> DriverRoot a) :
    forall {a : Addr},
      DriverRootUnion WorkerRoot SafepointRoot EnvAnchor DispatchAnchor a ->
      DriverRoot a := by
  intro a hroot
  cases hroot with
  | inl hworker => exact workerIncluded hworker
  | inr rest =>
      cases rest with
      | inl hsafepoint => exact safepointIncluded hsafepoint
      | inr rest =>
          cases rest with
          | inl henv => exact envIncluded henv
          | inr hdispatch => exact dispatchIncluded hdispatch

theorem driver_root_union_survives_collection
    {WorkerRoot SafepointRoot EnvAnchor DispatchAnchor
      DriverRoot Marked Freed : Addr -> Prop}
    (workerIncluded : forall {a : Addr}, WorkerRoot a -> DriverRoot a)
    (safepointIncluded : forall {a : Addr}, SafepointRoot a -> DriverRoot a)
    (envIncluded : forall {a : Addr}, EnvAnchor a -> DriverRoot a)
    (dispatchIncluded : forall {a : Addr}, DispatchAnchor a -> DriverRoot a)
    (markFromDriverRoots : forall {a : Addr}, DriverRoot a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr},
      DriverRootUnion WorkerRoot SafepointRoot EnvAnchor DispatchAnchor a ->
      Not (Freed a) := by
  intro a hroot hfreed
  have hdriver :
      DriverRoot a :=
    driver_root_union_complete
      (WorkerRoot := WorkerRoot)
      (SafepointRoot := SafepointRoot)
      (EnvAnchor := EnvAnchor)
      (DispatchAnchor := DispatchAnchor)
      (DriverRoot := DriverRoot)
      workerIncluded
      safepointIncluded
      envIncluded
      dispatchIncluded
      hroot
  have hmarked := markFromDriverRoots hdriver
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.DriverRootUnion
