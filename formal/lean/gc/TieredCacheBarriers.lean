/-!
Tiered compilation cache rooting and SATB deletion obligations.

The global tiered compilation cache is persistent E0. It contributes pending
bytecode compile source roots and constants reachable from ready bytecode
chunks. Removed pending roots and cleared compiled constants must be shaded
during an active E2 SATB mark before they become unreachable from the cache.

Pending root guards carry an ownership token. A guard may unregister only the
entry carrying its token, so an older guard cannot remove a newer pending root
registered for the same expression hash.
-/

namespace MeTTaTron.GC.TieredCacheBarriers

variable {Addr : Type u} {Entry : Type v} {Token : Type w}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def ConcurrentCollectorRoot
    (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
    (a : Addr) : Prop :=
  InitialRoot a ∨ DriverRoot a ∨ ShadedDeletion a ∨ AllocateBlack a

def TieredCacheRegisteredValue
    (PendingRoot CompiledConstant : Addr -> Prop)
    (a : Addr) : Prop :=
  PendingRoot a ∨ CompiledConstant a

def TieredCacheRemovedValue
    (PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
      ClearPendingVictim ClearCompiledConstant : Addr -> Prop)
    (a : Addr) : Prop :=
  PendingOverwriteVictim a ∨
  PendingCancelVictim a ∨
  PendingGuardDropVictim a ∨
  ClearPendingVictim a ∨
  ClearCompiledConstant a

def TokenCheckedRemove
    (EntryToken : Entry -> Token)
    (guard : Token)
    (entry : Entry) : Prop :=
  EntryToken entry = guard

theorem old_pending_guard_cannot_remove_newer_root
    {EntryToken : Entry -> Token}
    {oldEntry newEntry : Entry}
    {oldGuard : Token}
    (_oldMatches : EntryToken oldEntry = oldGuard)
    (newDoesNotMatch : EntryToken newEntry ≠ oldGuard) :
    ¬ TokenCheckedRemove EntryToken oldGuard newEntry := by
  intro removed
  exact newDoesNotMatch removed

theorem tiered_cache_value_is_structural_root
    {PendingRoot CompiledConstant PendingScanned CompiledScanned
      StructuralRoot : Addr -> Prop}
    (scanPending : ∀ {a}, PendingRoot a -> PendingScanned a)
    (rootPending : ∀ {a}, PendingScanned a -> StructuralRoot a)
    (scanCompiled : ∀ {a}, CompiledConstant a -> CompiledScanned a)
    (rootCompiled : ∀ {a}, CompiledScanned a -> StructuralRoot a) :
    ∀ {a},
      TieredCacheRegisteredValue PendingRoot CompiledConstant a ->
      StructuralRoot a := by
  intro a registered
  cases registered with
  | inl pending => exact rootPending (scanPending pending)
  | inr compiled => exact rootCompiled (scanCompiled compiled)

theorem tiered_cache_value_survives_collection
    {PendingRoot CompiledConstant PendingScanned CompiledScanned
      StructuralRoot Marked Freed : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    (scanPending : ∀ {a}, PendingRoot a -> PendingScanned a)
    (rootPending : ∀ {a}, PendingScanned a -> StructuralRoot a)
    (scanCompiled : ∀ {a}, CompiledConstant a -> CompiledScanned a)
    (rootCompiled : ∀ {a}, CompiledScanned a -> StructuralRoot a)
    (markComplete : ∀ {a}, Reach StructuralRoot Edge a -> Marked a)
    (sweepOnlyUnmarked : ∀ {a}, Freed a -> ¬ Marked a) :
    ∀ {a},
      TieredCacheRegisteredValue PendingRoot CompiledConstant a ->
      ¬ Freed a := by
  intro a registered freed
  have hroot : StructuralRoot a := by
    cases registered with
    | inl pending => exact rootPending (scanPending pending)
    | inr compiled => exact rootCompiled (scanCompiled compiled)
  have marked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked freed marked

theorem removed_tiered_cache_value_shaded
    {PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
      ClearPendingVictim ClearCompiledConstant ShadedDeletion : Addr -> Prop}
    (overwrite : ∀ {a}, PendingOverwriteVictim a -> ShadedDeletion a)
    (cancel : ∀ {a}, PendingCancelVictim a -> ShadedDeletion a)
    (guardDrop : ∀ {a}, PendingGuardDropVictim a -> ShadedDeletion a)
    (clearPending : ∀ {a}, ClearPendingVictim a -> ShadedDeletion a)
    (clearCompiled : ∀ {a}, ClearCompiledConstant a -> ShadedDeletion a) :
    ∀ {a},
      TieredCacheRemovedValue
        PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
        ClearPendingVictim ClearCompiledConstant a ->
      ShadedDeletion a := by
  intro a removed
  cases removed with
  | inl overwriteVictim => exact overwrite overwriteVictim
  | inr rest =>
      cases rest with
      | inl cancelVictim => exact cancel cancelVictim
      | inr rest =>
          cases rest with
          | inl guardVictim => exact guardDrop guardVictim
          | inr rest =>
              cases rest with
              | inl clearPendingVictim => exact clearPending clearPendingVictim
              | inr clearCompiledVictim => exact clearCompiled clearCompiledVictim

theorem removed_tiered_cache_value_survives_satb_collection
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack
      PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
      ClearPendingVictim ClearCompiledConstant SnapshotLive : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (overwrite : ∀ {a}, PendingOverwriteVictim a -> ShadedDeletion a)
    (cancel : ∀ {a}, PendingCancelVictim a -> ShadedDeletion a)
    (guardDrop : ∀ {a}, PendingGuardDropVictim a -> ShadedDeletion a)
    (clearPending : ∀ {a}, ClearPendingVictim a -> ShadedDeletion a)
    (clearCompiled : ∀ {a}, ClearCompiledConstant a -> ShadedDeletion a)
    (removed :
      ∀ {a},
        SnapshotLive a ->
        TieredCacheRemovedValue
          PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
          ClearPendingVictim ClearCompiledConstant a)
    (markComplete :
      ∀ {a}, Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
        Marked a)
    (sweepOnlyUnmarked : ∀ {a}, Freed a -> ¬ Marked a) :
    ∀ {a}, SnapshotLive a -> ¬ Freed a := by
  intro a live freed
  have shaded : ShadedDeletion a := by
    cases removed live with
    | inl overwriteVictim => exact overwrite overwriteVictim
    | inr rest =>
        cases rest with
        | inl cancelVictim => exact cancel cancelVictim
        | inr rest =>
            cases rest with
            | inl guardVictim => exact guardDrop guardVictim
            | inr rest =>
                cases rest with
                | inl clearPendingVictim => exact clearPending clearPendingVictim
                | inr clearCompiledVictim => exact clearCompiled clearCompiledVictim
  have root : ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a :=
    Or.inr (Or.inr (Or.inl shaded))
  have marked := markComplete (Reach.root root)
  exact sweepOnlyUnmarked freed marked

end MeTTaTron.GC.TieredCacheBarriers
