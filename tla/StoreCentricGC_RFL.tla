-------------------------- MODULE StoreCentricGC_RFL --------------------------
(***************************************************************************)
(* Sequence-level model of the R-FL free-list bug and fix.                  *)
(*                                                                         *)
(* The older generational models intentionally abstracted freeList as a set *)
(* and therefore could not express the current bug: repeated sweeps of the  *)
(* always-young current segment appended the same dead slot twice before it *)
(* was popped. This model keeps freeList as a sequence, matching Rust's Vec *)
(* LIFO behavior closely enough to prove the production free_bit lifecycle: *)
(*                                                                         *)
(*   freeBit(s) is set iff s occurs in freeList.                            *)
(*                                                                         *)
(* It also models release of a non-current young segment. The Rust release  *)
(* path drops the segment bitmaps, so it must first drain any listed slots  *)
(* for that segment; otherwise the concrete "bit iff listed" invariant is  *)
(* broken as soon as the segment storage is released.                       *)
(*                                                                         *)
(* FixApplied = FALSE models the pre-fix sweep push: every dead slot scan   *)
(* appends, even if that slot is already listed. TLC must find a duplicate. *)
(* FixApplied = TRUE models the fix: the sweep appends only when freeBit    *)
(* was clear; pop clears freeBit before reuse/discard.                      *)
(***************************************************************************)

EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    SLOTS,        \* finite set of slots in the current segment
    FixApplied    \* TRUE = free_bit idempotent push; FALSE = pre-fix bug

VARIABLES
    slotState,    \* SLOTS -> {"live", "dead", "released"}
    freeList,     \* sequence of SLOTS, Rust Vec order; pop uses the tail
    freeBit       \* SUBSET SLOTS, production membership bitmap abstraction

vars == <<slotState, freeList, freeBit>>

SeqContains(seq, x) ==
    \E i \in 1..Len(seq) : seq[i] = x

RECURSIVE RemoveAll(_, _)
RemoveAll(seq, x) ==
    IF Len(seq) = 0
    THEN <<>>
    ELSE IF seq[1] = x
         THEN RemoveAll(Tail(seq), x)
         ELSE <<seq[1]>> \o RemoveAll(Tail(seq), x)

Init ==
    /\ slotState = [s \in SLOTS |-> "dead"]
    /\ freeList = <<>>
    /\ freeBit = {}

SweepCurrentSlot ==
    \E s \in SLOTS :
        /\ slotState[s] = "dead"
        /\ IF FixApplied
           THEN IF s \in freeBit
                THEN UNCHANGED <<slotState, freeList, freeBit>>
                ELSE /\ freeList' = Append(freeList, s)
                     /\ freeBit' = freeBit \cup {s}
                     /\ UNCHANGED slotState
           ELSE /\ freeList' = Append(freeList, s)
                /\ freeBit' = freeBit
                /\ UNCHANGED slotState

PopForReuseOrDiscard ==
    /\ Len(freeList) > 0
    /\ LET s == freeList[Len(freeList)] IN
       /\ slotState[s] # "released"
       /\ freeList' = SubSeq(freeList, 1, Len(freeList) - 1)
       /\ freeBit' = freeBit \ {s}
       /\ slotState' = [slotState EXCEPT ![s] = "live"]

DropReusedSlot ==
    \E s \in SLOTS :
        /\ slotState[s] = "live"
        /\ slotState' = [slotState EXCEPT ![s] = "dead"]
        /\ UNCHANGED <<freeList, freeBit>>

ReleaseDeadSlot ==
    \E s \in SLOTS :
        /\ slotState[s] = "dead"
        /\ slotState' = [slotState EXCEPT ![s] = "released"]
        /\ IF FixApplied
           THEN /\ freeList' = RemoveAll(freeList, s)
                /\ freeBit' = freeBit \ {s}
           ELSE /\ freeList' = freeList
                /\ freeBit' = freeBit \ {s}

AllReleased ==
    \A s \in SLOTS : slotState[s] = "released"

Next ==
    \/ SweepCurrentSlot
    \/ PopForReuseOrDiscard
    \/ DropReusedSlot
    \/ ReleaseDeadSlot
    \/ /\ AllReleased
       /\ UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ slotState \in [SLOTS -> {"live", "dead", "released"}]
    /\ freeList \in Seq(SLOTS)
    /\ freeBit \subseteq SLOTS
    /\ FixApplied \in BOOLEAN

NoDuplicateFreeListEntries ==
    \A i, j \in 1..Len(freeList) :
        (i # j) => freeList[i] # freeList[j]

FreeBitExact ==
    \A s \in SLOTS :
        (s \in freeBit) <=> SeqContains(freeList, s)

=============================================================================
