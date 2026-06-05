/-!
E2 allocate-black publication obligations.

During concurrent SATB marking, a freshly allocated slot must be marked before
publication makes it visible to the marker/sweeper. The source coupling pins
that order in IndexArena; this proof discharges the abstract safety shape used
by the SATB theorem.
-/

namespace MeTTaTron.GC.AllocateBlack

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def SATBRoot
    (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
    (a : Addr) : Prop :=
  InitialRoot a ∨ DriverRoot a ∨ ShadedDeletion a ∨ AllocateBlack a

theorem published_alloc_is_satb_root
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack PublishedAlloc : Addr -> Prop}
    (publishedBlack : forall {a : Addr}, PublishedAlloc a -> AllocateBlack a) :
    forall {a : Addr},
      PublishedAlloc a ->
      SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a := by
  intro a hpublished
  exact Or.inr (Or.inr (Or.inr (publishedBlack hpublished)))

theorem published_alloc_survives_collection
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack PublishedAlloc : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (publishedBlack : forall {a : Addr}, PublishedAlloc a -> AllocateBlack a)
    (markComplete :
      forall {a : Addr}, Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, PublishedAlloc a -> Not (Freed a) := by
  intro a hpublished hfreed
  have hroot : SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a :=
    published_alloc_is_satb_root
      (InitialRoot := InitialRoot)
      (DriverRoot := DriverRoot)
      (ShadedDeletion := ShadedDeletion)
      (AllocateBlack := AllocateBlack)
      (PublishedAlloc := PublishedAlloc)
      publishedBlack
      hpublished
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

theorem mark_before_publish_survives_direct_sweep
    {PublishedAlloc Marked Freed : Addr -> Prop}
    (publishedMarked : forall {a : Addr}, PublishedAlloc a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, PublishedAlloc a -> Not (Freed a) := by
  intro a hpublished hfreed
  have hmarked := publishedMarked hpublished
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.AllocateBlack
