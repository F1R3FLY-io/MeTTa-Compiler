------------------------- MODULE SATBFinalRemarkPremarked -------------------------
(***************************************************************************)
(* E2 SATB final-remark premarked-root discriminator.                      *)
(*                                                                        *)
(* Allocate-black can publish a final-rendezvous root with its mark bit    *)
(* already set. A final remark that uses the mark bit as its traversal     *)
(* deduplication set will not descend through that root, so a white child  *)
(* reachable only from the premarked root can be swept. The implementation *)
(* must instead revisit final roots with a separate traversal-seen set.     *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT RevisitPremarkedRoots

VARIABLES
    rootMarked,
    childMarked,
    childFreed,
    phase

vars == <<rootMarked, childMarked, childFreed, phase>>

TypeOK ==
    /\ rootMarked \in BOOLEAN
    /\ childMarked \in BOOLEAN
    /\ childFreed \in BOOLEAN
    /\ phase \in {"final", "remarked", "swept"}

Init ==
    /\ rootMarked = TRUE
    /\ childMarked = FALSE
    /\ childFreed = FALSE
    /\ phase = "final"

FinalRemark ==
    /\ phase = "final"
    /\ phase' = "remarked"
    /\ IF RevisitPremarkedRoots
       THEN childMarked' = TRUE
       ELSE childMarked' = childMarked
    /\ UNCHANGED <<rootMarked, childFreed>>

Sweep ==
    /\ phase = "remarked"
    /\ phase' = "swept"
    /\ childFreed' = ~childMarked
    /\ UNCHANGED <<rootMarked, childMarked>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ FinalRemark
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoReachableChildFreed ==
    ~childFreed

=============================================================================
