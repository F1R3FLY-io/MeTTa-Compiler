-------------------------- MODULE ConservativeMinorMark --------------------------
(*****************************************************************************)
(* Discriminator for the C1 minor-mark source obligation after first-class    *)
(* mutable SpaceHandle values entered the index heap.                        *)
(*                                                                           *)
(* The old young-only minor was sound only under NoOldToYoungEdge. A Space    *)
(* value breaks that premise without violating σ-node immutability: an OLD    *)
(* SpaceHandle can be a reachable container whose current contents include a  *)
(* YOUNG value. The minor must therefore traverse OLD reachable nodes while   *)
(* still setting mark bits only on YOUNG nodes.                               *)
(*                                                                           *)
(* TraceOldDuringMinor = TRUE models the fixed IndexHeap::mark_young.         *)
(* TraceOldDuringMinor = FALSE models the skipped-old algorithm and must      *)
(* produce a live-young miss.                                                 *)
(*****************************************************************************)

EXTENDS FiniteSets

CONSTANT TraceOldDuringMinor

OldRoot == "old-space"
YoungChild == "young-atom"
Addr == {OldRoot, YoungChild}
Young == {YoungChild}
Root == {OldRoot}
Edges(node) == IF node = OldRoot THEN {YoungChild} ELSE {}

VARIABLES
    seen,       \* nodes traversed by the minor marker
    marked,     \* young nodes whose mark bit is set
    phase       \* {"mutating", "marking", "sweeping"}

vars == <<seen, marked, phase>>

Init ==
    /\ seen = {}
    /\ marked = {}
    /\ phase = "mutating"

BeginMark ==
    /\ phase = "mutating"
    /\ seen' = Root
    /\ marked' = Root \cap Young
    /\ phase' = "marking"

CanTrace(parent) == TraceOldDuringMinor \/ parent \in Young

TraceEnabled ==
    \E parent \in seen :
      /\ CanTrace(parent)
      /\ \E child \in Edges(parent) : child \notin seen

TraceStep ==
    /\ phase = "marking"
    /\ \E parent \in seen :
      /\ CanTrace(parent)
      /\ \E child \in Edges(parent) :
        /\ child \notin seen
        /\ seen' = seen \union {child}
        /\ marked' = IF child \in Young THEN marked \union {child} ELSE marked
    /\ UNCHANGED phase

MarkComplete ==
    /\ phase = "marking"
    /\ ~TraceEnabled
    /\ phase' = "sweeping"
    /\ UNCHANGED <<seen, marked>>

Next == BeginMark \/ TraceStep \/ MarkComplete

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ seen \subseteq Addr
    /\ marked \subseteq Young
    /\ phase \in {"mutating", "marking", "sweeping"}

YoungReachableMarked ==
    phase = "sweeping" => YoungChild \in marked

=============================================================================
