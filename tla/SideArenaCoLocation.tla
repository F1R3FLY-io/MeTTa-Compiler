-------------------------- MODULE SideArenaCoLocation --------------------------
(***************************************************************************)
(* Co-location obligation for variable-length index-GC nodes.             *)
(*                                                                        *)
(* The node stores only a side-column index. Readers choose the side       *)
(* segment from the node address, so a side-bearing node is readable only  *)
(* when its side payload was interned in that same segment before the node *)
(* was published.                                                         *)
(*                                                                        *)
(* SameSegment = TRUE        => production co-location.                   *)
(* SameSegment = FALSE       => side payload in the wrong segment.         *)
(* InternBeforePublish=TRUE  => production publication order.             *)
(* InternBeforePublish=FALSE => node can publish before side payload.      *)
(***************************************************************************)

CONSTANTS SameSegment, InternBeforePublish

VARIABLES
    phase,
    sideReady,
    nodePublished,
    readObserved,
    readOk

vars == <<phase, sideReady, nodePublished, readObserved, readOk>>

TypeOK ==
    /\ SameSegment \in BOOLEAN
    /\ InternBeforePublish \in BOOLEAN
    /\ phase \in {"start", "side", "published", "read"}
    /\ sideReady \in BOOLEAN
    /\ nodePublished \in BOOLEAN
    /\ readObserved \in BOOLEAN
    /\ readOk \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ sideReady = FALSE
    /\ nodePublished = FALSE
    /\ readObserved = FALSE
    /\ readOk = TRUE

InternSide ==
    /\ ~sideReady
    /\ sideReady' = TRUE
    /\ phase' = "side"
    /\ UNCHANGED <<nodePublished, readObserved, readOk>>

PublishNode ==
    /\ ~nodePublished
    /\ IF InternBeforePublish THEN sideReady ELSE TRUE
    /\ nodePublished' = TRUE
    /\ phase' = "published"
    /\ UNCHANGED <<sideReady, readObserved, readOk>>

ReadNode ==
    /\ nodePublished
    /\ ~readObserved
    /\ readObserved' = TRUE
    /\ readOk' = (sideReady /\ SameSegment)
    /\ phase' = "read"
    /\ UNCHANGED <<sideReady, nodePublished>>

Done ==
    /\ readObserved
    /\ UNCHANGED vars

Next ==
    \/ InternSide
    \/ PublishNode
    \/ ReadNode
    \/ Done

Spec == Init /\ [][Next]_vars

NoBadSideRead ==
    ~(readObserved /\ ~readOk)

=============================================================================
