/-!
Global space-registry rooting and SATB deletion obligations.

`GLOBAL_SPACE_REGISTRY` is part of persistent E0. Registered spaces are read
structurally by `collect_global_anchors` through `collect_all_gc_values`. During
E2 SATB marking, replaced, removed, and bulk-cleared `SpaceHandle`s must shade
the values reachable from their old handles while the SATB phase gate is held.
This proof captures those two source-coupled shapes.
-/

namespace MeTTaTron.GC.SpaceRegistryBarriers

variable {Addr : Type u} {Space : Type v}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def ConcurrentCollectorRoot
    (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
    (a : Addr) : Prop :=
  InitialRoot a ∨ DriverRoot a ∨ ShadedDeletion a ∨ AllocateBlack a

def SpaceRegistryRemovedValue
    (OverwriteVictim RemoveVictim ClearVictim : Addr -> Prop)
    (a : Addr) : Prop :=
  OverwriteVictim a ∨ RemoveVictim a ∨ ClearVictim a

theorem registered_space_value_is_structural_root
    {Registered Scanned : Space -> Prop}
    {SpaceValue : Space -> Addr -> Prop}
    {StructuralRoot : Addr -> Prop}
    (scan : ∀ s, Registered s -> Scanned s)
    (root : ∀ s a, Scanned s -> SpaceValue s a -> StructuralRoot a) :
    ∀ s a, Registered s -> SpaceValue s a -> StructuralRoot a := by
  intro s a registered value
  exact root s a (scan s registered) value

theorem registered_space_value_survives_collection
    {Registered Scanned : Space -> Prop}
    {SpaceValue : Space -> Addr -> Prop}
    {StructuralRoot Marked Freed : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    (scan : ∀ s, Registered s -> Scanned s)
    (root : ∀ s a, Scanned s -> SpaceValue s a -> StructuralRoot a)
    (markComplete : ∀ {a}, Reach StructuralRoot Edge a -> Marked a)
    (sweepOnlyUnmarked : ∀ {a}, Freed a -> ¬ Marked a) :
    ∀ s a, Registered s -> SpaceValue s a -> ¬ Freed a := by
  intro s a registered value freed
  have hroot : StructuralRoot a :=
    registered_space_value_is_structural_root scan root s a registered value
  have marked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked freed marked

theorem removed_space_registry_value_shaded
    {OverwriteVictim RemoveVictim ClearVictim ShadedDeletion : Addr -> Prop}
    (overwrite : ∀ {a}, OverwriteVictim a -> ShadedDeletion a)
    (remove : ∀ {a}, RemoveVictim a -> ShadedDeletion a)
    (clear : ∀ {a}, ClearVictim a -> ShadedDeletion a) :
    ∀ {a},
      SpaceRegistryRemovedValue OverwriteVictim RemoveVictim ClearVictim a ->
      ShadedDeletion a := by
  intro a removed
  cases removed with
  | inl overwriteVictim => exact overwrite overwriteVictim
  | inr rest =>
      cases rest with
      | inl removeVictim => exact remove removeVictim
      | inr clearVictim => exact clear clearVictim

theorem removed_space_registry_value_survives_satb_collection
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack
      OverwriteVictim RemoveVictim ClearVictim SnapshotLive : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (overwrite : ∀ {a}, OverwriteVictim a -> ShadedDeletion a)
    (remove : ∀ {a}, RemoveVictim a -> ShadedDeletion a)
    (clear : ∀ {a}, ClearVictim a -> ShadedDeletion a)
    (removed :
      ∀ {a},
        SnapshotLive a ->
        SpaceRegistryRemovedValue OverwriteVictim RemoveVictim ClearVictim a)
    (markComplete :
      ∀ {a}, Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
        Marked a)
    (sweepOnlyUnmarked : ∀ {a}, Freed a -> ¬ Marked a) :
    ∀ {a}, SnapshotLive a -> ¬ Freed a := by
  intro a live freed
  have shaded : ShadedDeletion a := by
    cases removed live with
    | inl overwriteVictim =>
        exact overwrite overwriteVictim
    | inr rest =>
        cases rest with
        | inl removeVictim => exact remove removeVictim
        | inr clearVictim => exact clear clearVictim
  have root : ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a :=
    Or.inr (Or.inr (Or.inl shaded))
  have marked := markComplete (Reach.root root)
  exact sweepOnlyUnmarked freed marked

end MeTTaTron.GC.SpaceRegistryBarriers
