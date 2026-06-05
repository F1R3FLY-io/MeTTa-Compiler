--------------------------- MODULE AllocateBlackPublish ---------------------------
(***************************************************************************)
(* E2 allocate-black publication-order discriminator.                      *)
(*                                                                        *)
(* During concurrent marking, a freshly allocated node can become visible  *)
(* to the marker/sweeper only after its slot is published. Allocate-black  *)
(* requires setting the mark bit before publication. If publication comes  *)
(* first, a sweep can observe a published but unmarked new node and reclaim *)
(* it.                                                                    *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT MarkBeforePublish

VARIABLES
    published,
    marked,
    freed,
    phase

vars == <<published, marked, freed, phase>>

TypeOK ==
    /\ published \in BOOLEAN
    /\ marked \in BOOLEAN
    /\ freed \in BOOLEAN
    /\ phase \in {"allocating", "marking", "swept"}

Init ==
    /\ published = FALSE
    /\ marked = FALSE
    /\ freed = FALSE
    /\ phase = "allocating"

MarkBefore ==
    /\ phase = "allocating"
    /\ MarkBeforePublish
    /\ ~marked
    /\ marked' = TRUE
    /\ UNCHANGED <<published, freed, phase>>

Publish ==
    /\ phase = "allocating"
    /\ ~published
    /\ IF MarkBeforePublish THEN marked ELSE TRUE
    /\ published' = TRUE
    /\ phase' = "marking"
    /\ UNCHANGED <<marked, freed>>

LateMark ==
    /\ phase = "marking"
    /\ ~MarkBeforePublish
    /\ published
    /\ ~marked
    /\ marked' = TRUE
    /\ UNCHANGED <<published, freed, phase>>

Sweep ==
    /\ phase = "marking"
    /\ published
    /\ phase' = "swept"
    /\ freed' = ~marked
    /\ UNCHANGED <<published, marked>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ MarkBefore
    \/ Publish
    \/ LateMark
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoPublishedAllocSwept ==
    ~(published /\ freed)

=============================================================================
