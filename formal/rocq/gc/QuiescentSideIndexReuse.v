(** Quiescent side-column index reuse.

    Node-slot reuse alone does not bound the CESK/index allocator: variable-
    length side payload columns must also stop appending once a full true-
    quiescence drain has proved an old side payload dead.  This model captures
    the source-coupled contract implemented by [SideColumn::free] and
    [SideColumn::push]:

    - only full true-quiescence drains that consumed the reclaim snapshot may
      place an index on the reusable stack;
    - rendezvous/midloop/deferred-pending states cannot make an index reusable;
    - a push that observes reusable pressure consumes one reusable index and
      preserves the side-column high-water.
*)

From Stdlib Require Import Arith.Arith.
From Stdlib Require Import micromega.Lia.

Module MeTTaTron_GC_QuiescentSideIndexReuse.

Section QuiescentSideIndexReuseModel.
  Variable SideIndex : Type.

  Record SideColumnState := {
    published_len : nat;
    reusable_len : nat;
    bump_len : nat
  }.

  Definition FreeAddsReusable (before after : SideColumnState) : Prop :=
    published_len after = published_len before /\
    bump_len after = bump_len before /\
    reusable_len after = S (reusable_len before).

  Definition IdempotentFreeNoop (before after : SideColumnState) : Prop :=
    published_len after = published_len before /\
    bump_len after = bump_len before /\
    reusable_len after = reusable_len before.

  Definition PushReuses (before after : SideColumnState) : Prop :=
    reusable_len before > 0 /\
    published_len after = published_len before /\
    bump_len after = bump_len before /\
    reusable_len after = Nat.pred (reusable_len before).

  Definition PushBumps (before after : SideColumnState) : Prop :=
    reusable_len before = 0 /\
    published_len after = S (published_len before) /\
    bump_len after = S (bump_len before) /\
    reusable_len after = 0.

  Definition PushPolicy (before after : SideColumnState) : Prop :=
    (reusable_len before > 0 -> PushReuses before after) /\
    (reusable_len before = 0 -> PushBumps before after).

  Definition ReusableIndexSafe
      (Reusable Freed Quiescent FullMark SnapshotConsumed DeferredPending :
          SideIndex -> Prop) : Prop :=
    forall idx,
      Reusable idx ->
      Freed idx /\ Quiescent idx /\ FullMark idx /\
        SnapshotConsumed idx /\ ~ DeferredPending idx.

  Theorem reusable_index_implies_quiescent_full_consumed :
    forall Reusable Freed Quiescent FullMark SnapshotConsumed DeferredPending idx,
      ReusableIndexSafe Reusable Freed Quiescent FullMark SnapshotConsumed
        DeferredPending ->
      Reusable idx ->
      Quiescent idx /\ FullMark idx /\ SnapshotConsumed idx.
  Proof.
    intros Reusable Freed Quiescent FullMark SnapshotConsumed DeferredPending
           idx Hsafe Hreusable.
    destruct (Hsafe idx Hreusable) as
      [_ [Hquiescent [Hfull [Hconsumed _]]]].
    repeat split; assumption.
  Qed.

  Theorem deferred_pending_index_is_not_reusable :
    forall Reusable Freed Quiescent FullMark SnapshotConsumed DeferredPending idx,
      ReusableIndexSafe Reusable Freed Quiescent FullMark SnapshotConsumed
        DeferredPending ->
      DeferredPending idx ->
      ~ Reusable idx.
  Proof.
    intros Reusable Freed Quiescent FullMark SnapshotConsumed DeferredPending
           idx Hsafe Hdeferred Hreusable.
    destruct (Hsafe idx Hreusable) as [_ [_ [_ [_ Hnot_deferred]]]].
    apply Hnot_deferred.
    exact Hdeferred.
  Qed.

  Theorem nonquiescent_index_is_not_reusable :
    forall Reusable Freed Quiescent FullMark SnapshotConsumed DeferredPending idx,
      ReusableIndexSafe Reusable Freed Quiescent FullMark SnapshotConsumed
        DeferredPending ->
      ~ Quiescent idx ->
      ~ Reusable idx.
  Proof.
    intros Reusable Freed Quiescent FullMark SnapshotConsumed DeferredPending
           idx Hsafe Hnot_quiescent Hreusable.
    destruct (Hsafe idx Hreusable) as [_ [Hquiescent _]].
    apply Hnot_quiescent.
    exact Hquiescent.
  Qed.

  Theorem push_reuse_preserves_side_high_water :
    forall before after,
      PushReuses before after ->
      published_len after = published_len before /\
      bump_len after = bump_len before.
  Proof.
    intros before after Hreuse.
    destruct Hreuse as [_ [Hpublished [Hbump _]]].
    split; assumption.
  Qed.

  Theorem push_policy_reuses_under_pressure :
    forall before after,
      PushPolicy before after ->
      reusable_len before > 0 ->
      PushReuses before after.
  Proof.
    intros before after Hpolicy Hpressure.
    destruct Hpolicy as [Hreuse _].
    apply Hreuse.
    exact Hpressure.
  Qed.

  Theorem free_then_push_reuses_without_bump :
    forall before freed after,
      FreeAddsReusable before freed ->
      PushPolicy freed after ->
      published_len after = published_len before /\
      bump_len after = bump_len before /\
      reusable_len after = reusable_len before.
  Proof.
    intros before freed after Hfree Hpolicy.
    destruct Hfree as [Hpub_free [Hbump_free Hreuse_free]].
    pose proof (push_policy_reuses_under_pressure
                  freed after Hpolicy) as Hpush.
    assert (Hpressure : reusable_len freed > 0) by lia.
    specialize (Hpush Hpressure).
    destruct Hpush as [_ [Hpub_push [Hbump_push Hreuse_push]]].
    repeat split.
    - rewrite Hpub_push. exact Hpub_free.
    - rewrite Hbump_push. exact Hbump_free.
    - rewrite Hreuse_push. rewrite Hreuse_free. simpl. reflexivity.
  Qed.

  Theorem idempotent_free_cannot_create_duplicate_reusable_index :
    forall before after,
      IdempotentFreeNoop before after ->
      reusable_len after = reusable_len before.
  Proof.
    intros before after Hnoop.
    destruct Hnoop as [_ [_ Hreuse]].
    exact Hreuse.
  Qed.

  Theorem bump_only_when_no_reusable_index :
    forall before after,
      PushPolicy before after ->
      PushBumps before after ->
      reusable_len before = 0.
  Proof.
    intros before after _ Hbumps.
    destruct Hbumps as [Hnone _].
    exact Hnone.
  Qed.

  Theorem reusable_pressure_excludes_bump_path :
    forall before after,
      PushPolicy before after ->
      reusable_len before > 0 ->
      ~ PushBumps before after.
  Proof.
    intros before after _ Hpressure Hbumps.
    destruct Hbumps as [Hnone _].
    lia.
  Qed.
End QuiescentSideIndexReuseModel.

End MeTTaTron_GC_QuiescentSideIndexReuse.
