/-!
Driver-C publication obligation for the CESK index collector.

`MettaState.source/output` are caller-held control roots. During an eval
transition they must be published into the narrow driver/safepoint channel used
by midloop and rendezvous collection. Once published, ordinary root-complete
marking and sweep safety retain them.
-/

namespace MeTTaTron.GC.DriverCPublication

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def CollectorRoot
    (StructuralRoot DriverRoot : Addr -> Prop)
    (a : Addr) : Prop :=
  StructuralRoot a ∨ DriverRoot a

theorem published_driver_c_is_collector_root
    {DriverC DriverRoot StructuralRoot : Addr -> Prop}
    (published : forall {a : Addr}, DriverC a -> DriverRoot a) :
    forall {a : Addr}, DriverC a -> CollectorRoot StructuralRoot DriverRoot a := by
  intro a hdriver
  exact Or.inr (published hdriver)

theorem published_driver_c_survives_sweep
    {DriverC DriverRoot StructuralRoot : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (published : forall {a : Addr}, DriverC a -> DriverRoot a)
    (markComplete :
      forall {a : Addr}, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, DriverC a -> Not (Freed a) := by
  intro a hdriver hfreed
  have hroot : CollectorRoot StructuralRoot DriverRoot a :=
    published_driver_c_is_collector_root
      (DriverC := DriverC)
      (DriverRoot := DriverRoot)
      (StructuralRoot := StructuralRoot)
      published
      hdriver
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.DriverCPublication
