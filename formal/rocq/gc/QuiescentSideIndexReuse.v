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

(** ===== Generation guard: the span/child/byte side-index ABA fix =====

    The reuse model above counts indices but treats a side index as an opaque
    identity over time: its [ReusableIndexSafe] contract ASSUMES a reusable index
    already had its reclaim snapshot consumed ([SnapshotConsumed]). That
    abstraction cannot express the use-after-free that the free-list reuse
    introduced: a STALE [SideReclaim] snapshot {idx} (captured from a dead node
    slot whose bytes still name [idx]) can be drained AFTER [idx] was freed and
    re-handed by [SideColumn::push] to a LIVE node — so the same [idx] is named by
    both a dead snapshot and a live occupant, and a [free] keyed on [idx] ALONE
    drops the live occupant's payload (the "live Spanned slot" panic / a true UAF;
    gdb-confirmed: dead owner Addr(2) vs live reuser Addr(220) on span idx 43).

    The fix stamps every [SideColumn::push] with a fresh, strictly-increasing
    per-cell GENERATION (source: index_heap.rs `slot.0 = slot.0.wrapping_add(1)`),
    stores it in the node's ref ([SpanRef]/[ChildRef]/[ByteRef] `{idx, gen}`),
    captures it in [SideReclaim], and frees the cell ONLY when the cell's current
    generation equals the snapshot's captured generation ([SideColumn::free(idx,
    gen)]). This section models a single side cell as a succession of OCCUPANTS,
    each interned at a DISTINCT generation (push bumps it), and proves the guard
    never frees a live occupant. (Generations are idealized as [nat]: the source
    [u32] would only alias after 2^32 reuses of ONE cell — unreachable in any run
    — so [gen_injective] is the sound abstraction of the strictly-monotone bump.) *)

Section GenerationGuardSafety.
  (* An occupant = one [push]'s tenant of a fixed side cell. *)
  Variable Occupant : Type.
  (* The generation each occupant was interned at. Distinct occupants of one cell
     have DISTINCT generations: every [SideColumn::push] increments the cell's
     generation, so it is strictly monotone across the cell's tenancy — hence
     [gen_of] is injective on this cell. *)
  Variable gen_of : Occupant -> nat.
  Hypothesis gen_injective :
    forall o1 o2, gen_of o1 = gen_of o2 -> o1 = o2.

  (* Whether an occupant is a LIVE node (reachable / marked by the collector). *)
  Variable live : Occupant -> Prop.

  (* The cell's CURRENT occupant; the cell's current generation is [gen_of current]
     (the most recent [push] set it). *)
  Variable current : Occupant.

  (* The implemented guard ([SideColumn::free]): a deferred-free snapshot taken
     from occupant [o] drops the cell iff [o]'s captured generation equals the
     cell's current generation. *)
  Definition GuardDrops (o : Occupant) : Prop := gen_of o = gen_of current.

  (* MAIN SAFETY: a guarded free driven by a DEAD-owner snapshot never drops a
     LIVE occupant. A [SideReclaim] snapshot is captured only from a slot the
     sweep RECLAIMED (a dead occupant [o]); if the guard fires
     ([gen_of o = gen_of current]), injectivity forces [o = current], so the
     current occupant IS that dead owner — not live. *)
  Theorem gen_guard_never_frees_live :
    forall o, ~ live o -> GuardDrops o -> ~ live current.
  Proof.
    intros o Hdead Hguard. unfold GuardDrops in Hguard.
    rewrite <- (gen_injective o current Hguard). exact Hdead.
  Qed.

  (* Contrapositive: while the cell's current occupant is LIVE, NO dead-owner
     snapshot can free it — the guard never fires for it. *)
  Theorem live_current_survives_stale_free :
    forall o, ~ live o -> live current -> ~ GuardDrops o.
  Proof.
    intros o Hdead Hlive Hguard.
    exact (gen_guard_never_frees_live o Hdead Hguard Hlive).
  Qed.

  (* The reuse case is exactly a generation mismatch: an index reused since the
     snapshot has [gen_of o < gen_of current] (strictly older), so the guard
     SKIPS it — precisely the case the bug mishandled. *)
  Theorem reused_index_snapshot_does_not_drop :
    forall o, gen_of o < gen_of current -> ~ GuardDrops o.
  Proof.
    intros o Hlt Hguard. unfold GuardDrops in Hguard. lia.
  Qed.

  (* The PRE-FIX free, keyed on the bare index alone, drops whatever occupies the
     cell regardless of generation. *)
  Definition IdxOnlyDrops (_ : Occupant) : Prop := True.

  (* NON-VACUITY: the index-only free DROPS the cell for a stale dead-owner
     snapshot even when the current occupant is LIVE and the index was reused
     ([gen_of o < gen_of current]) — committing the use-after-free — whereas the
     generation guard does NOT. So the generation field is load-bearing: the guard
     removes exactly the unsafe drops the unguarded free performed. *)
  Theorem idx_only_free_drops_live_reuser :
    forall o, ~ live o -> live current -> gen_of o < gen_of current ->
      IdxOnlyDrops o /\ ~ GuardDrops o.
  Proof.
    intros o _ _ Hlt. split.
    - exact I.
    - unfold GuardDrops. lia.
  Qed.
End GenerationGuardSafety.

End MeTTaTron_GC_QuiescentSideIndexReuse.
