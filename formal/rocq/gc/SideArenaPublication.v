(** Side-arena publication obligations for variable-length index-GC nodes.

    Variable-length payloads live in side columns. A reader may follow a node's
    side reference only after the owning node is published. The side directory
    and side entry protocols therefore need the usual write-before-publish
    shape: page before chunk, chunk before entry, and entry write before entry
    publish.
*)

Module MeTTaTron_GC_SideArenaPublication.

Section SideArenaPublicationModel.
  Variable Ref : Type.

  Definition PublishedEntryReady
      (PageReady ChunkReady EntryWritten EntryPublished : Ref -> Prop)
      (r : Ref) : Prop :=
    PageReady r /\ ChunkReady r /\ EntryWritten r /\ EntryPublished r.

  Definition PublishedNodeSideReady
      (node_seg side_seg : nat)
      (SideReady NodePublished : Prop) : Prop :=
    NodePublished /\ SideReady /\ side_seg = node_seg.

  Theorem published_entry_has_ready_payload :
    forall (PagePublished ChunkPublished PageReady ChunkReady EntryWritten
            EntryPublished : Ref -> Prop),
      (forall r, EntryPublished r -> EntryWritten r) ->
      (forall r, EntryPublished r -> ChunkPublished r) ->
      (forall r, ChunkPublished r -> PagePublished r) ->
      (forall r, PagePublished r -> PageReady r) ->
      (forall r, ChunkPublished r -> ChunkReady r) ->
      forall r,
        EntryPublished r ->
        PublishedEntryReady PageReady ChunkReady EntryWritten EntryPublished r.
  Proof.
    intros PagePublished ChunkPublished PageReady ChunkReady EntryWritten
           EntryPublished Hwrite Hchunk Hpage Hpage_ready Hchunk_ready r Hentry.
    split.
    - apply Hpage_ready. apply Hpage. apply Hchunk. exact Hentry.
    - split.
      + apply Hchunk_ready. apply Hchunk. exact Hentry.
      + split.
        * apply Hwrite. exact Hentry.
        * exact Hentry.
  Qed.

  Theorem read_published_entry_has_ready_payload :
    forall (PagePublished ChunkPublished PageReady ChunkReady EntryWritten
            EntryPublished ReadObserved : Ref -> Prop),
      (forall r, ReadObserved r -> EntryPublished r) ->
      (forall r, EntryPublished r -> EntryWritten r) ->
      (forall r, EntryPublished r -> ChunkPublished r) ->
      (forall r, ChunkPublished r -> PagePublished r) ->
      (forall r, PagePublished r -> PageReady r) ->
      (forall r, ChunkPublished r -> ChunkReady r) ->
      forall r,
        ReadObserved r ->
        PublishedEntryReady PageReady ChunkReady EntryWritten EntryPublished r.
  Proof.
    intros PagePublished ChunkPublished PageReady ChunkReady EntryWritten
           EntryPublished ReadObserved Hread Hwrite Hchunk Hpage
           Hpage_ready Hchunk_ready r Hread_observed.
    apply (published_entry_has_ready_payload
             PagePublished ChunkPublished PageReady ChunkReady EntryWritten
             EntryPublished Hwrite Hchunk Hpage Hpage_ready Hchunk_ready r).
    apply Hread.
    exact Hread_observed.
  Qed.

  Theorem published_side_bearing_node_reads_ready_payload :
    forall (node_seg side_seg : nat) (SideReady NodePublished : Prop),
      (NodePublished -> SideReady) ->
      (NodePublished -> side_seg = node_seg) ->
      NodePublished ->
      PublishedNodeSideReady node_seg side_seg SideReady NodePublished.
  Proof.
    intros node_seg side_seg SideReady NodePublished Hside Hsame Hpublished.
    split.
    - exact Hpublished.
    - split.
      + apply Hside. exact Hpublished.
      + apply Hsame. exact Hpublished.
  Qed.

  Theorem wrong_segment_cannot_satisfy_published_node_side_ready :
    forall (node_seg side_seg : nat) (SideReady NodePublished : Prop),
      side_seg <> node_seg ->
      ~ PublishedNodeSideReady node_seg side_seg SideReady NodePublished.
  Proof.
    intros node_seg side_seg SideReady NodePublished Hwrong Hready.
    destruct Hready as [_ [_ Hsame]].
    apply Hwrong.
    exact Hsame.
  Qed.
End SideArenaPublicationModel.

End MeTTaTron_GC_SideArenaPublication.
