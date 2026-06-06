(** Thread-local subgoal/thunk table rooting and SATB deletion obligations.

    The CESK subgoal and thunk tables are thread-local persistent roots.  A
    mutator publishes them through the canonical structural root reader, which
    calls `collect_subgoal_roots` and `collect_thunk_roots` from
    `collect_global_anchors`.  During E2 SATB marking, stale evictions,
    overwrites, explicit removals, invalidation/full clears, and thunk result
    replacement must shade the removed result values while the SATB phase gate is
    held.
*)

Module MeTTaTron_GC_ThreadLocalTablesBarriers.

Section ThreadLocalTablesBarrierModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition ConcurrentCollectorRoot
      (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition ThreadLocalTableRegisteredValue
      (SubgoalResult ThunkResult : Addr -> Prop)
      (a : Addr) : Prop :=
    SubgoalResult a \/ ThunkResult a.

  Definition ThreadLocalTableRemovedValue
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
    ThunkReplaceVictim a.

  Theorem thread_local_table_value_is_structural_root :
    forall (SubgoalResult ThunkResult SubgoalScanned ThunkScanned
            StructuralRoot : Addr -> Prop),
      (forall a, SubgoalResult a -> SubgoalScanned a) ->
      (forall a, SubgoalScanned a -> StructuralRoot a) ->
      (forall a, ThunkResult a -> ThunkScanned a) ->
      (forall a, ThunkScanned a -> StructuralRoot a) ->
      forall a,
        ThreadLocalTableRegisteredValue SubgoalResult ThunkResult a ->
        StructuralRoot a.
  Proof.
    intros SubgoalResult ThunkResult SubgoalScanned ThunkScanned StructuralRoot
           Hscan_subgoal Hroot_subgoal Hscan_thunk Hroot_thunk a Hregistered.
    destruct Hregistered as [Hsubgoal | Hthunk].
    - apply Hroot_subgoal.
      apply Hscan_subgoal.
      exact Hsubgoal.
    - apply Hroot_thunk.
      apply Hscan_thunk.
      exact Hthunk.
  Qed.

  Theorem thread_local_table_value_survives_collection :
    forall (SubgoalResult ThunkResult SubgoalScanned ThunkScanned
            StructuralRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, SubgoalResult a -> SubgoalScanned a) ->
      (forall a, SubgoalScanned a -> StructuralRoot a) ->
      (forall a, ThunkResult a -> ThunkScanned a) ->
      (forall a, ThunkScanned a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        ThreadLocalTableRegisteredValue SubgoalResult ThunkResult a ->
        ~ Freed a.
  Proof.
    intros SubgoalResult ThunkResult SubgoalScanned ThunkScanned
           StructuralRoot Marked Freed Edge Hscan_subgoal Hroot_subgoal
           Hscan_thunk Hroot_thunk Hmark Hsweep a Hregistered Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    destruct Hregistered as [Hsubgoal | Hthunk].
    - apply Hroot_subgoal.
      apply Hscan_subgoal.
      exact Hsubgoal.
    - apply Hroot_thunk.
      apply Hscan_thunk.
      exact Hthunk.
  Qed.

  Theorem removed_thread_local_table_value_shaded :
    forall (SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
            SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
            ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim
            ShadedDeletion : Addr -> Prop),
      (forall a, SubgoalStaleVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalRemoveVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalClearVictim a -> ShadedDeletion a) ->
      (forall a, ThunkStaleVictim a -> ShadedDeletion a) ->
      (forall a, ThunkOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, ThunkRemoveVictim a -> ShadedDeletion a) ->
      (forall a, ThunkClearVictim a -> ShadedDeletion a) ->
      (forall a, ThunkReplaceVictim a -> ShadedDeletion a) ->
      forall a,
        ThreadLocalTableRemovedValue
          SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
          SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
          ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim a ->
        ShadedDeletion a.
  Proof.
    intros SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
           SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
           ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim ShadedDeletion
           Hsubgoal_stale Hsubgoal_overwrite Hsubgoal_remove Hsubgoal_clear
           Hthunk_stale Hthunk_overwrite Hthunk_remove Hthunk_clear Hthunk_replace
           a Hremoved.
    destruct Hremoved as
        [Hsubgoal_stale_a |
         [Hsubgoal_overwrite_a |
          [Hsubgoal_remove_a |
           [Hsubgoal_clear_a |
            [Hthunk_stale_a |
             [Hthunk_overwrite_a |
              [Hthunk_remove_a |
               [Hthunk_clear_a | Hthunk_replace_a]]]]]]]].
    - apply Hsubgoal_stale. exact Hsubgoal_stale_a.
    - apply Hsubgoal_overwrite. exact Hsubgoal_overwrite_a.
    - apply Hsubgoal_remove. exact Hsubgoal_remove_a.
    - apply Hsubgoal_clear. exact Hsubgoal_clear_a.
    - apply Hthunk_stale. exact Hthunk_stale_a.
    - apply Hthunk_overwrite. exact Hthunk_overwrite_a.
    - apply Hthunk_remove. exact Hthunk_remove_a.
    - apply Hthunk_clear. exact Hthunk_clear_a.
    - apply Hthunk_replace. exact Hthunk_replace_a.
  Qed.

  Theorem removed_thread_local_table_value_survives_satb_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
            SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
            ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim
            SnapshotLive : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, SubgoalStaleVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalRemoveVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalClearVictim a -> ShadedDeletion a) ->
      (forall a, ThunkStaleVictim a -> ShadedDeletion a) ->
      (forall a, ThunkOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, ThunkRemoveVictim a -> ShadedDeletion a) ->
      (forall a, ThunkClearVictim a -> ShadedDeletion a) ->
      (forall a, ThunkReplaceVictim a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          ThreadLocalTableRemovedValue
            SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
            SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
            ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        SnapshotLive a ->
        ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
           SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
           ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim SnapshotLive
           Edge Marked Freed Hsubgoal_stale Hsubgoal_overwrite Hsubgoal_remove
           Hsubgoal_clear Hthunk_stale Hthunk_overwrite Hthunk_remove Hthunk_clear
           Hthunk_replace Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as
        [Hsubgoal_stale_a |
         [Hsubgoal_overwrite_a |
          [Hsubgoal_remove_a |
           [Hsubgoal_clear_a |
            [Hthunk_stale_a |
             [Hthunk_overwrite_a |
              [Hthunk_remove_a |
               [Hthunk_clear_a | Hthunk_replace_a]]]]]]]].
    - apply Hsubgoal_stale. exact Hsubgoal_stale_a.
    - apply Hsubgoal_overwrite. exact Hsubgoal_overwrite_a.
    - apply Hsubgoal_remove. exact Hsubgoal_remove_a.
    - apply Hsubgoal_clear. exact Hsubgoal_clear_a.
    - apply Hthunk_stale. exact Hthunk_stale_a.
    - apply Hthunk_overwrite. exact Hthunk_overwrite_a.
    - apply Hthunk_remove. exact Hthunk_remove_a.
    - apply Hthunk_clear. exact Hthunk_clear_a.
    - apply Hthunk_replace. exact Hthunk_replace_a.
  Qed.
End ThreadLocalTablesBarrierModel.

End MeTTaTron_GC_ThreadLocalTablesBarriers.
