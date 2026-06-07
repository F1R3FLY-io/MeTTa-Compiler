------------------------ MODULE ConcurrentBumpFreshOnly ------------------------
(***************************************************************************)
(* D-RLOCK/B2 concurrent allocation fresh-only model.                     *)
(*                                                                        *)
(* Concurrent shared allocation must return only fresh bump slots.         *)
(* Free-list reuse is exclusive/quiescent only.                            *)
(*                                                                        *)
(* ConcurrentUsesFreeList=TRUE => concurrent allocation may pop reuse.     *)
(* ReuseRequiresExclusive=FALSE => reuse may run without exclusivity.      *)
(***************************************************************************)

CONSTANTS ConcurrentUsesFreeList, ReuseRequiresExclusive

VARIABLES
    phase,
    mode,
    exclusive,
    returnedFresh,
    returnedFreeList

vars == <<phase, mode, exclusive, returnedFresh, returnedFreeList>>

Modes == {"concurrent", "reuse"}

TypeOK ==
    /\ ConcurrentUsesFreeList \in BOOLEAN
    /\ ReuseRequiresExclusive \in BOOLEAN
    /\ phase \in {"start", "chosen", "returned"}
    /\ mode \in Modes
    /\ exclusive \in BOOLEAN
    /\ returnedFresh \in BOOLEAN
    /\ returnedFreeList \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ mode = "concurrent"
    /\ exclusive = FALSE
    /\ returnedFresh = FALSE
    /\ returnedFreeList = FALSE

PickMode ==
    /\ phase = "start"
    /\ mode' \in Modes
    /\ exclusive' \in BOOLEAN
    /\ ~(mode' = "reuse" /\ ReuseRequiresExclusive /\ ~exclusive')
    /\ phase' = "chosen"
    /\ UNCHANGED <<returnedFresh, returnedFreeList>>

ReturnSlot ==
    /\ phase = "chosen"
    /\ IF mode = "concurrent" THEN
          /\ returnedFresh' = ~ConcurrentUsesFreeList
          /\ returnedFreeList' = ConcurrentUsesFreeList
       ELSE
          /\ returnedFresh' = FALSE
          /\ returnedFreeList' = TRUE
    /\ phase' = "returned"
    /\ UNCHANGED <<mode, exclusive>>

Done ==
    /\ phase = "returned"
    /\ UNCHANGED vars

Next ==
    \/ PickMode
    \/ ReturnSlot
    \/ Done

Spec == Init /\ [][Next]_vars

ConcurrentNeverReturnsFreeList ==
    ~(phase = "returned" /\ mode = "concurrent" /\ returnedFreeList)

ConcurrentReturnsFresh ==
    ~(phase = "returned" /\ mode = "concurrent" /\ ~returnedFresh)

ReuseOnlyExclusive ==
    ~(phase = "returned" /\ mode = "reuse" /\ returnedFreeList /\ ~exclusive)

NoFreshAndFreeListAlias ==
    ~(phase = "returned" /\ returnedFresh /\ returnedFreeList)

=============================================================================
