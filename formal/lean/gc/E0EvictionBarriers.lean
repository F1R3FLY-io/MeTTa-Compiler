/-!
E2 SATB E0 cache eviction and bulk-clear obligations.

Value-bearing E0 caches can remove snapshot-live values through capacity
eviction, same-key overwrite, or bulk clear. The source-coupling harness pins
each concrete cache path to expose the removed pre-image and shade it while the
SATB phase gate is held. This proof discharges the abstract safety shape
consumed by the concurrent SATB theorem.
-/

namespace MeTTaTron.GC.E0EvictionBarriers

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def SATBRoot
    (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
    (a : Addr) : Prop :=
  InitialRoot a ∨ FinalDriverRoot a ∨ ShadedDeletion a ∨ AllocateBlack a

def E0CacheRemovedPreimage
    (CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop)
    (a : Addr) : Prop :=
  CapacityVictim a ∨ OverwriteVictim a ∨ BulkClearedEntry a

theorem e0_cache_removed_preimage_shaded
    {CapacityVictim OverwriteVictim BulkClearedEntry ShadedDeletion : Addr -> Prop}
    (capacityShaded : forall {a : Addr}, CapacityVictim a -> ShadedDeletion a)
    (overwriteShaded : forall {a : Addr}, OverwriteVictim a -> ShadedDeletion a)
    (bulkShaded : forall {a : Addr}, BulkClearedEntry a -> ShadedDeletion a) :
    forall {a : Addr},
      E0CacheRemovedPreimage CapacityVictim OverwriteVictim BulkClearedEntry a ->
      ShadedDeletion a := by
  intro a hremoved
  cases hremoved with
  | inl hcapacity => exact capacityShaded hcapacity
  | inr hrest =>
      cases hrest with
      | inl hoverwrite => exact overwriteShaded hoverwrite
      | inr hbulk => exact bulkShaded hbulk

theorem capacity_evicted_victim_is_satb_root
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
      CapacityVictim : Addr -> Prop}
    (capacityShaded : forall {a : Addr}, CapacityVictim a -> ShadedDeletion a) :
    forall {a : Addr},
      CapacityVictim a ->
      SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a := by
  intro a hvictim
  exact Or.inr (Or.inr (Or.inl (capacityShaded hvictim)))

theorem bulk_cleared_entry_is_satb_root
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
      BulkClearedEntry : Addr -> Prop}
    (bulkShaded : forall {a : Addr}, BulkClearedEntry a -> ShadedDeletion a) :
    forall {a : Addr},
      BulkClearedEntry a ->
      SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a := by
  intro a hentry
  exact Or.inr (Or.inr (Or.inl (bulkShaded hentry)))

theorem e0_cache_removed_preimage_is_satb_root
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
      CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop}
    (capacityShaded : forall {a : Addr}, CapacityVictim a -> ShadedDeletion a)
    (overwriteShaded : forall {a : Addr}, OverwriteVictim a -> ShadedDeletion a)
    (bulkShaded : forall {a : Addr}, BulkClearedEntry a -> ShadedDeletion a) :
    forall {a : Addr},
      E0CacheRemovedPreimage CapacityVictim OverwriteVictim BulkClearedEntry a ->
      SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a := by
  intro a hremoved
  exact
    Or.inr
      (Or.inr
        (Or.inl
          (e0_cache_removed_preimage_shaded
            (CapacityVictim := CapacityVictim)
            (OverwriteVictim := OverwriteVictim)
            (BulkClearedEntry := BulkClearedEntry)
            (ShadedDeletion := ShadedDeletion)
            capacityShaded
            overwriteShaded
            bulkShaded
            hremoved)))

theorem capacity_evicted_victim_survives_collection
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
      CapacityVictim : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (capacityShaded : forall {a : Addr}, CapacityVictim a -> ShadedDeletion a)
    (markComplete :
      forall {a : Addr}, Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, CapacityVictim a -> Not (Freed a) := by
  intro a hvictim hfreed
  have hroot : SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a :=
    capacity_evicted_victim_is_satb_root
      (InitialRoot := InitialRoot)
      (FinalDriverRoot := FinalDriverRoot)
      (ShadedDeletion := ShadedDeletion)
      (AllocateBlack := AllocateBlack)
      (CapacityVictim := CapacityVictim)
      capacityShaded
      hvictim
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

theorem bulk_cleared_entry_survives_collection
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
      BulkClearedEntry : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (bulkShaded : forall {a : Addr}, BulkClearedEntry a -> ShadedDeletion a)
    (markComplete :
      forall {a : Addr}, Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, BulkClearedEntry a -> Not (Freed a) := by
  intro a hentry hfreed
  have hroot : SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a :=
    bulk_cleared_entry_is_satb_root
      (InitialRoot := InitialRoot)
      (FinalDriverRoot := FinalDriverRoot)
      (ShadedDeletion := ShadedDeletion)
      (AllocateBlack := AllocateBlack)
      (BulkClearedEntry := BulkClearedEntry)
      bulkShaded
      hentry
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

theorem e0_cache_removed_snapshot_live_survives_collection
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
      CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed SnapshotLive : Addr -> Prop}
    (capacityShaded : forall {a : Addr}, CapacityVictim a -> ShadedDeletion a)
    (overwriteShaded : forall {a : Addr}, OverwriteVictim a -> ShadedDeletion a)
    (bulkShaded : forall {a : Addr}, BulkClearedEntry a -> ShadedDeletion a)
    (removedCoverage :
      forall {a : Addr},
        SnapshotLive a ->
        E0CacheRemovedPreimage CapacityVictim OverwriteVictim BulkClearedEntry a)
    (markComplete :
      forall {a : Addr}, Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, SnapshotLive a -> Not (Freed a) := by
  intro a hlive hfreed
  have hroot : SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a :=
    e0_cache_removed_preimage_is_satb_root
      (InitialRoot := InitialRoot)
      (FinalDriverRoot := FinalDriverRoot)
      (ShadedDeletion := ShadedDeletion)
      (AllocateBlack := AllocateBlack)
      (CapacityVictim := CapacityVictim)
      (OverwriteVictim := OverwriteVictim)
      (BulkClearedEntry := BulkClearedEntry)
      capacityShaded
      overwriteShaded
      bulkShaded
      (removedCoverage hlive)
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.E0EvictionBarriers
