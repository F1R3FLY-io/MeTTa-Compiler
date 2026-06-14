---------------------------- MODULE BindingProjection ----------------------------
(***************************************************************************)
(* Conditional binding sidecar projection at ProcessRuleMatches boundaries. *)
(*                                                                         *)
(* The tracked branch boundary must keep the result/tracked seeds and the   *)
(* bound freshened dependencies reachable from visible values, while not    *)
(* carrying unrelated stale freshened rule-epoch keys into the parent       *)
(* result slot.  When no tracked/consumer context exists, the boundary      *)
(* preserves the full sidecar and defers projection to a later consumer.    *)
(***************************************************************************)
EXTENDS FiniteSets

CONSTANTS
    IncludeClosure,
    ProjectionContext,
    ProjectAtTrackedBoundary

VARIABLE phase

vars == <<phase>>

AllKeys == {"result", "tracked", "liveFresh", "staleFresh"}
FreshKeys == {"liveFresh", "staleFresh"}
Seeds == {"result", "tracked"}
ReferencedByVisible == {"liveFresh"}

Keep ==
    IF IncludeClosure
    THEN Seeds \cup ReferencedByVisible
    ELSE Seeds

Projected ==
    IF ProjectionContext /\ ProjectAtTrackedBoundary
    THEN Keep
    ELSE AllKeys

TypeOK ==
    /\ IncludeClosure \in BOOLEAN
    /\ ProjectionContext \in BOOLEAN
    /\ ProjectAtTrackedBoundary \in BOOLEAN
    /\ phase \in {"check"}
    /\ Keep \subseteq AllKeys
    /\ Projected \subseteq AllKeys

Init == phase = "check"

Next == UNCHANGED vars

Spec == Init /\ [][Next]_vars

LiveSeedsRetained ==
    Seeds \subseteq Projected

NoDanglingVisibleFreshRef ==
    "result" \in Projected => ReferencedByVisible \subseteq Projected

NoStaleFreshRetainedAtTrackedBoundary ==
    ProjectionContext => Projected \cap FreshKeys \subseteq ReferencedByVisible

NoContextDefersProjection ==
    ~ProjectionContext => Projected = AllKeys

Inv ==
    /\ TypeOK
    /\ LiveSeedsRetained
    /\ NoDanglingVisibleFreshRef
    /\ NoStaleFreshRetainedAtTrackedBoundary
    /\ NoContextDefersProjection

=============================================================================
