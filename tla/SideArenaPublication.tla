------------------------- MODULE SideArenaPublication -------------------------
(***************************************************************************)
(* Publication order for side-arena payloads of variable-length index-GC    *)
(* nodes. Positive config enforces page -> chunk -> entry-write -> entry    *)
(* publish. Negative configs allow each historical bad order.               *)
(***************************************************************************)

CONSTANTS
    PageBeforeChunk,
    ChunkBeforeEntry,
    WriteBeforeEntry

VARIABLES
    pageReady,
    pagePublished,
    chunkReady,
    chunkPublished,
    entryWritten,
    entryPublished,
    readObserved

vars ==
    <<pageReady, pagePublished, chunkReady, chunkPublished,
      entryWritten, entryPublished, readObserved>>

TypeOK ==
    /\ PageBeforeChunk \in BOOLEAN
    /\ ChunkBeforeEntry \in BOOLEAN
    /\ WriteBeforeEntry \in BOOLEAN
    /\ pageReady \in BOOLEAN
    /\ pagePublished \in BOOLEAN
    /\ chunkReady \in BOOLEAN
    /\ chunkPublished \in BOOLEAN
    /\ entryWritten \in BOOLEAN
    /\ entryPublished \in BOOLEAN
    /\ readObserved \in BOOLEAN

Init ==
    /\ pageReady = FALSE
    /\ pagePublished = FALSE
    /\ chunkReady = FALSE
    /\ chunkPublished = FALSE
    /\ entryWritten = FALSE
    /\ entryPublished = FALSE
    /\ readObserved = FALSE

PublishPage ==
    /\ ~pagePublished
    /\ pageReady' = TRUE
    /\ pagePublished' = TRUE
    /\ UNCHANGED <<chunkReady, chunkPublished, entryWritten,
                  entryPublished, readObserved>>

PublishChunk ==
    /\ ~chunkPublished
    /\ IF PageBeforeChunk THEN pagePublished ELSE TRUE
    /\ chunkReady' = TRUE
    /\ chunkPublished' = TRUE
    /\ UNCHANGED <<pageReady, pagePublished, entryWritten,
                  entryPublished, readObserved>>

WriteEntry ==
    /\ ~entryWritten
    /\ IF ChunkBeforeEntry THEN chunkPublished ELSE TRUE
    /\ entryWritten' = TRUE
    /\ UNCHANGED <<pageReady, pagePublished, chunkReady, chunkPublished,
                  entryPublished, readObserved>>

PublishEntry ==
    /\ ~entryPublished
    /\ IF WriteBeforeEntry THEN entryWritten ELSE TRUE
    /\ entryPublished' = TRUE
    /\ UNCHANGED <<pageReady, pagePublished, chunkReady, chunkPublished,
                  entryWritten, readObserved>>

ReadEntry ==
    /\ entryPublished
    /\ ~readObserved
    /\ readObserved' = TRUE
    /\ UNCHANGED <<pageReady, pagePublished, chunkReady, chunkPublished,
                  entryWritten, entryPublished>>

Done ==
    /\ readObserved
    /\ UNCHANGED vars

Next ==
    \/ PublishPage
    \/ PublishChunk
    \/ WriteEntry
    \/ PublishEntry
    \/ ReadEntry
    \/ Done

Spec == Init /\ [][Next]_vars

PublishedEntryReady ==
    readObserved =>
      /\ pageReady
      /\ pagePublished
      /\ chunkReady
      /\ chunkPublished
      /\ entryWritten
      /\ entryPublished

=============================================================================
