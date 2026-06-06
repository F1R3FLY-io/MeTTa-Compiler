/-!
Thread-local subgoal/thunk table rooting and SATB deletion obligations.

The CESK subgoal and thunk tables are thread-local persistent roots. A mutator
publishes them through the canonical structural root reader, which calls
`collect_subgoal_roots` and `collect_thunk_roots` from `collect_global_anchors`.
During E2 SATB marking, stale evictions, overwrites, explicit removals,
invalidation/full clears, and thunk result replacement must shade the removed
result values while the SATB phase gate is held.
-/

namespace MeTTaTron.GC.ThreadLocalTablesBarriers

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def ConcurrentCollectorRoot
    (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
    (a : Addr) : Prop :=
  InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a

def ThreadLocalTableRegisteredValue
    (SubgoalResult ThunkResult : Addr -> Prop)
    (a : Addr) : Prop :=
  SubgoalResult a \/ ThunkResult a

def ThreadLocalTableRemovedValue
    (SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
      SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
      ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim : Addr -> Prop)
    (a : Addr) : Prop :=
  SubgoalStaleVictim a \/
  SubgoalOverwriteVictim a \/
  SubgoalRemoveVictim a \/
  SubgoalClearVictim a \/
  ThunkStaleVictim a \/
  ThunkOverwriteVictim a \/
  ThunkRemoveVictim a \/
  ThunkClearVictim a \/
  ThunkReplaceVictim a

theorem thread_local_table_value_is_structural_root
    {SubgoalResult ThunkResult SubgoalScanned ThunkScanned
      StructuralRoot : Addr -> Prop}
    (scanSubgoal : forall {a}, SubgoalResult a -> SubgoalScanned a)
    (rootSubgoal : forall {a}, SubgoalScanned a -> StructuralRoot a)
    (scanThunk : forall {a}, ThunkResult a -> ThunkScanned a)
    (rootThunk : forall {a}, ThunkScanned a -> StructuralRoot a) :
    forall {a},
      ThreadLocalTableRegisteredValue SubgoalResult ThunkResult a ->
      StructuralRoot a := by
  intro a registered
  cases registered with
  | inl subgoal => exact rootSubgoal (scanSubgoal subgoal)
  | inr thunk => exact rootThunk (scanThunk thunk)

theorem thread_local_table_value_survives_collection
    {SubgoalResult ThunkResult SubgoalScanned ThunkScanned
      StructuralRoot Marked Freed : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    (scanSubgoal : forall {a}, SubgoalResult a -> SubgoalScanned a)
    (rootSubgoal : forall {a}, SubgoalScanned a -> StructuralRoot a)
    (scanThunk : forall {a}, ThunkResult a -> ThunkScanned a)
    (rootThunk : forall {a}, ThunkScanned a -> StructuralRoot a)
    (markComplete : forall {a}, Reach StructuralRoot Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a}, Freed a -> Not (Marked a)) :
    forall {a},
      ThreadLocalTableRegisteredValue SubgoalResult ThunkResult a ->
      Not (Freed a) := by
  intro a registered freed
  have hroot : StructuralRoot a := by
    cases registered with
    | inl subgoal => exact rootSubgoal (scanSubgoal subgoal)
    | inr thunk => exact rootThunk (scanThunk thunk)
  have marked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked freed marked

theorem removed_thread_local_table_value_shaded
    {SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
      SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
      ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim
      ShadedDeletion : Addr -> Prop}
    (subgoalStale : forall {a}, SubgoalStaleVictim a -> ShadedDeletion a)
    (subgoalOverwrite : forall {a}, SubgoalOverwriteVictim a -> ShadedDeletion a)
    (subgoalRemove : forall {a}, SubgoalRemoveVictim a -> ShadedDeletion a)
    (subgoalClear : forall {a}, SubgoalClearVictim a -> ShadedDeletion a)
    (thunkStale : forall {a}, ThunkStaleVictim a -> ShadedDeletion a)
    (thunkOverwrite : forall {a}, ThunkOverwriteVictim a -> ShadedDeletion a)
    (thunkRemove : forall {a}, ThunkRemoveVictim a -> ShadedDeletion a)
    (thunkClear : forall {a}, ThunkClearVictim a -> ShadedDeletion a)
    (thunkReplace : forall {a}, ThunkReplaceVictim a -> ShadedDeletion a) :
    forall {a},
      ThreadLocalTableRemovedValue
        SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
        SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
        ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim a ->
      ShadedDeletion a := by
  intro a removed
  cases removed with
  | inl victim => exact subgoalStale victim
  | inr rest =>
      cases rest with
      | inl victim => exact subgoalOverwrite victim
      | inr rest =>
          cases rest with
          | inl victim => exact subgoalRemove victim
          | inr rest =>
              cases rest with
              | inl victim => exact subgoalClear victim
              | inr rest =>
                  cases rest with
                  | inl victim => exact thunkStale victim
                  | inr rest =>
                      cases rest with
                      | inl victim => exact thunkOverwrite victim
                      | inr rest =>
                          cases rest with
                          | inl victim => exact thunkRemove victim
                          | inr rest =>
                              cases rest with
                              | inl victim => exact thunkClear victim
                              | inr victim => exact thunkReplace victim

theorem removed_thread_local_table_value_survives_satb_collection
    {InitialRoot DriverRoot ShadedDeletion AllocateBlack
      SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
      SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
      ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim
      SnapshotLive : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (subgoalStale : forall {a}, SubgoalStaleVictim a -> ShadedDeletion a)
    (subgoalOverwrite : forall {a}, SubgoalOverwriteVictim a -> ShadedDeletion a)
    (subgoalRemove : forall {a}, SubgoalRemoveVictim a -> ShadedDeletion a)
    (subgoalClear : forall {a}, SubgoalClearVictim a -> ShadedDeletion a)
    (thunkStale : forall {a}, ThunkStaleVictim a -> ShadedDeletion a)
    (thunkOverwrite : forall {a}, ThunkOverwriteVictim a -> ShadedDeletion a)
    (thunkRemove : forall {a}, ThunkRemoveVictim a -> ShadedDeletion a)
    (thunkClear : forall {a}, ThunkClearVictim a -> ShadedDeletion a)
    (thunkReplace : forall {a}, ThunkReplaceVictim a -> ShadedDeletion a)
    (removed :
      forall {a},
        SnapshotLive a ->
        ThreadLocalTableRemovedValue
          SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
          SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
          ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim a)
    (markComplete :
      forall {a}, Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
        Marked a)
    (sweepOnlyUnmarked : forall {a}, Freed a -> Not (Marked a)) :
    forall {a}, SnapshotLive a -> Not (Freed a) := by
  intro a live freed
  have shaded : ShadedDeletion a := by
    cases removed live with
    | inl victim => exact subgoalStale victim
    | inr rest =>
        cases rest with
        | inl victim => exact subgoalOverwrite victim
        | inr rest =>
            cases rest with
            | inl victim => exact subgoalRemove victim
            | inr rest =>
                cases rest with
                | inl victim => exact subgoalClear victim
                | inr rest =>
                    cases rest with
                    | inl victim => exact thunkStale victim
                    | inr rest =>
                        cases rest with
                        | inl victim => exact thunkOverwrite victim
                        | inr rest =>
                            cases rest with
                            | inl victim => exact thunkRemove victim
                            | inr rest =>
                                cases rest with
                                | inl victim => exact thunkClear victim
                                | inr victim => exact thunkReplace victim
  have root : ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a :=
    Or.inr (Or.inr (Or.inl shaded))
  have marked := markComplete (Reach.root root)
  exact sweepOnlyUnmarked freed marked

end MeTTaTron.GC.ThreadLocalTablesBarriers
