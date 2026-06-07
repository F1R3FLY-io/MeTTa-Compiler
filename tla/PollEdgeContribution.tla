-------------------------- MODULE PollEdgeContribution --------------------------
(***************************************************************************)
(* A depth-positive worker that reaches a GC-pending poll edge must collect, *)
(* drop its guard, publish roots, and only then wait for the rendezvous.     *)
(* This is the GC-facing poll-edge contract; it does not claim general       *)
(* scheduler fairness.                                                       *)
(***************************************************************************)

CONSTANTS
    IncludeCollect,
    IncludeDrop,
    IncludePublish,
    IncludeWait

VARIABLES
    phase,
    collected,
    dropped,
    published,
    waited

vars ==
    <<phase, collected, dropped, published, waited>>

Phases == {"edge", "collected", "dropped", "published_phase", "waiting"}

TypeOK ==
    /\ IncludeCollect \in BOOLEAN
    /\ IncludeDrop \in BOOLEAN
    /\ IncludePublish \in BOOLEAN
    /\ IncludeWait \in BOOLEAN
    /\ phase \in Phases
    /\ collected \in BOOLEAN
    /\ dropped \in BOOLEAN
    /\ published \in BOOLEAN
    /\ waited \in BOOLEAN

Init ==
    /\ phase = "edge"
    /\ collected = FALSE
    /\ dropped = FALSE
    /\ published = FALSE
    /\ waited = FALSE

CollectRoots ==
    /\ phase = "edge"
    /\ collected' = IncludeCollect
    /\ phase' = "collected"
    /\ UNCHANGED <<dropped, published, waited>>

DropGuard ==
    /\ phase = "collected"
    /\ dropped' = IncludeDrop
    /\ phase' = "dropped"
    /\ UNCHANGED <<collected, published, waited>>

PublishRoots ==
    /\ phase = "dropped"
    /\ published' = IncludePublish
    /\ phase' = "published_phase"
    /\ UNCHANGED <<collected, dropped, waited>>

WaitForResume ==
    /\ phase = "published_phase"
    /\ waited' = IncludeWait
    /\ phase' = "waiting"
    /\ UNCHANGED <<collected, dropped, published>>

Done ==
    /\ phase = "waiting"
    /\ UNCHANGED vars

Next ==
    \/ CollectRoots
    \/ DropGuard
    \/ PublishRoots
    \/ WaitForResume
    \/ Done

Spec == Init /\ [][Next]_vars

WaitAfterContribution ==
    waited => collected /\ dropped /\ published

PublishBeforeWait ==
    phase = "waiting" => published

=============================================================================
