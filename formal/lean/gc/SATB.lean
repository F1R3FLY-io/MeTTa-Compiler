/-!
Generic SATB deletion-barrier safety theorem for the E2 concurrent mark plan.

The theorem is intentionally parametric: it does not assume a finite heap or a
particular MeTTaTron data structure.  It states the collector obligation E2 must
realize in source: every value live in the snapshot-at-the-beginning set, or
captured by the final rendezvous, is reachable from either the initial roots,
the final driver roots, a SATB-shaded deletion pre-image, or an allocate-black
root before sweep.
-/

namespace MeTTaTron.GC.SATB

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def SATBRoot
    (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
    (a : Addr) : Prop :=
  InitialRoot a ∨ DriverRoot a ∨ ShadedDeletion a ∨ AllocateBlack a

theorem driver_root_is_satb_root
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop}
    {a : Addr} :
    DriverRoot a -> SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a := by
  intro h
  exact Or.inr (Or.inl h)

theorem deleted_preimage_is_satb_root
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop}
    {a : Addr} :
    ShadedDeletion a -> SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a := by
  intro h
  exact Or.inr (Or.inr (Or.inl h))

theorem allocate_black_is_satb_root
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop}
    {a : Addr} :
    AllocateBlack a -> SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a := by
  intro h
  exact Or.inr (Or.inr (Or.inr h))

theorem no_driver_root_uaf
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (markComplete :
      forall {a : Addr}, Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, DriverRoot a -> Not (Freed a) := by
  intro a hdriver hfreed
  have hreach : Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a :=
    Reach.root (driver_root_is_satb_root hdriver)
  have hmarked := markComplete hreach
  exact sweepOnlyUnmarked hfreed hmarked

theorem no_snapshot_live_uaf
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed SnapshotLive : Addr -> Prop}
    (markComplete :
      forall {a : Addr}, Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a))
    (snapshotLiveCovered :
      forall {a : Addr}, SnapshotLive a -> Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a) :
    forall {a : Addr}, SnapshotLive a -> Not (Freed a) := by
  intro a hlive hfreed
  have hreach := snapshotLiveCovered hlive
  have hmarked := markComplete hreach
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.SATB
