/-!
Side-arena publication obligations for variable-length index-GC nodes.

Variable-length payloads live in side columns. A reader may follow a node's side
reference only after the owning node is published. The side directory and side
entry protocols therefore need the usual write-before-publish shape: page before
chunk, chunk before entry, and entry write before entry publish.
-/

namespace MeTTaTron.GC.SideArenaPublication

variable {Ref : Type u}

def PublishedEntryReady
    (PageReady ChunkReady EntryWritten EntryPublished : Ref -> Prop)
    (r : Ref) : Prop :=
  PageReady r ∧ ChunkReady r ∧ EntryWritten r ∧ EntryPublished r

theorem published_entry_has_ready_payload
    {PagePublished ChunkPublished PageReady ChunkReady EntryWritten EntryPublished :
      Ref -> Prop}
    (entryPublishAfterWrite :
      forall {r : Ref}, EntryPublished r -> EntryWritten r)
    (entryPublishAfterChunk :
      forall {r : Ref}, EntryPublished r -> ChunkPublished r)
    (chunkPublishAfterPage :
      forall {r : Ref}, ChunkPublished r -> PagePublished r)
    (pagePublishReady :
      forall {r : Ref}, PagePublished r -> PageReady r)
    (chunkPublishReady :
      forall {r : Ref}, ChunkPublished r -> ChunkReady r) :
    forall {r : Ref},
      EntryPublished r ->
      PublishedEntryReady PageReady ChunkReady EntryWritten EntryPublished r := by
  intro r hentry
  have hchunk := entryPublishAfterChunk hentry
  have hpage := chunkPublishAfterPage hchunk
  exact And.intro
    (pagePublishReady hpage)
    (And.intro
      (chunkPublishReady hchunk)
      (And.intro (entryPublishAfterWrite hentry) hentry))

theorem read_published_entry_has_ready_payload
    {PagePublished ChunkPublished PageReady ChunkReady EntryWritten EntryPublished
      ReadObserved : Ref -> Prop}
    (readOnlyAfterPublish :
      forall {r : Ref}, ReadObserved r -> EntryPublished r)
    (entryPublishAfterWrite :
      forall {r : Ref}, EntryPublished r -> EntryWritten r)
    (entryPublishAfterChunk :
      forall {r : Ref}, EntryPublished r -> ChunkPublished r)
    (chunkPublishAfterPage :
      forall {r : Ref}, ChunkPublished r -> PagePublished r)
    (pagePublishReady :
      forall {r : Ref}, PagePublished r -> PageReady r)
    (chunkPublishReady :
      forall {r : Ref}, ChunkPublished r -> ChunkReady r) :
    forall {r : Ref},
      ReadObserved r ->
      PublishedEntryReady PageReady ChunkReady EntryWritten EntryPublished r := by
  intro r hread
  exact
    published_entry_has_ready_payload
      (PagePublished := PagePublished)
      (ChunkPublished := ChunkPublished)
      (PageReady := PageReady)
      (ChunkReady := ChunkReady)
      (EntryWritten := EntryWritten)
      (EntryPublished := EntryPublished)
      entryPublishAfterWrite
      entryPublishAfterChunk
      chunkPublishAfterPage
      pagePublishReady
      chunkPublishReady
      (readOnlyAfterPublish hread)

end MeTTaTron.GC.SideArenaPublication
