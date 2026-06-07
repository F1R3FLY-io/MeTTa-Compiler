------------------------ MODULE AbstractGCLiveNarrowing ------------------------
(***************************************************************************)
(* C2 abstract-GC live-field narrowing discriminator.                       *)
(*                                                                         *)
(* collect_live_values drops the three post-cut iterator fields from the K  *)
(* root set. This is sound only when those fields are dead for the next      *)
(* transition. A config that leaves any dropped field future-live must       *)
(* violate NoFutureTouchFreed after mark/sweep.                             *)
(***************************************************************************)
EXTENDS FiniteSets

CONSTANTS
    DeadFields

Fields ==
    {"kept", "remainingMatches", "remainingAlts", "remainingTemplates"}

DroppedFields ==
    {"remainingMatches", "remainingAlts", "remainingTemplates"}

LiveRoots ==
    Fields \ DroppedFields

FutureTouches ==
    Fields \ DeadFields

VARIABLES
    phase,
    marked,
    freed

vars == <<phase, marked, freed>>

TypeOK ==
    /\ DeadFields \subseteq Fields
    /\ phase \in {"start", "marked", "swept"}
    /\ marked \subseteq Fields
    /\ freed \subseteq Fields

Init ==
    /\ phase = "start"
    /\ marked = {}
    /\ freed = {}

MarkNarrowedRoots ==
    /\ phase = "start"
    /\ phase' = "marked"
    /\ marked' = LiveRoots
    /\ UNCHANGED freed

SweepUnmarked ==
    /\ phase = "marked"
    /\ phase' = "swept"
    /\ freed' = Fields \ marked
    /\ UNCHANGED marked

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ MarkNarrowedRoots
    \/ SweepUnmarked
    \/ Done

Spec == Init /\ [][Next]_vars

NoFutureTouchFreed ==
    FutureTouches \cap freed = {}

=============================================================================
