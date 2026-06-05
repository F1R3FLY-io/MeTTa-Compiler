-------------------------- MODULE SATBE0MutationSites --------------------------
(***************************************************************************)
(* E2 SATB source-obligation discriminator for value-bearing E0 substores. *)
(*                                                                        *)
(* The structural E0 reader traces several nested containers: spaces, the  *)
(* rule index, and mutable environment maps/tokens/state. If a snapshot-   *)
(* live value is deleted from any such container during concurrent marking, *)
(* Yuasa SATB requires the removed pre-image to be shaded.                 *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS UseSpaceBarrier, UseRuleBarrier, UseEnvBarrier

VARIABLES
    spaceRoot,
    ruleRoot,
    envRoot,
    spaceShaded,
    ruleShaded,
    envShaded,
    spaceMarked,
    ruleMarked,
    envMarked,
    spaceFreed,
    ruleFreed,
    envFreed,
    phase

vars ==
    <<spaceRoot, ruleRoot, envRoot,
      spaceShaded, ruleShaded, envShaded,
      spaceMarked, ruleMarked, envMarked,
      spaceFreed, ruleFreed, envFreed, phase>>

TypeOK ==
    /\ spaceRoot \in BOOLEAN
    /\ ruleRoot \in BOOLEAN
    /\ envRoot \in BOOLEAN
    /\ spaceShaded \in BOOLEAN
    /\ ruleShaded \in BOOLEAN
    /\ envShaded \in BOOLEAN
    /\ spaceMarked \in BOOLEAN
    /\ ruleMarked \in BOOLEAN
    /\ envMarked \in BOOLEAN
    /\ spaceFreed \in BOOLEAN
    /\ ruleFreed \in BOOLEAN
    /\ envFreed \in BOOLEAN
    /\ phase \in {"marking", "swept"}

Init ==
    /\ spaceRoot = TRUE
    /\ ruleRoot = TRUE
    /\ envRoot = TRUE
    /\ spaceShaded = FALSE
    /\ ruleShaded = FALSE
    /\ envShaded = FALSE
    /\ spaceMarked = FALSE
    /\ ruleMarked = FALSE
    /\ envMarked = FALSE
    /\ spaceFreed = FALSE
    /\ ruleFreed = FALSE
    /\ envFreed = FALSE
    /\ phase = "marking"

DeleteSpaceRoot ==
    /\ phase = "marking"
    /\ spaceRoot
    /\ spaceRoot' = FALSE
    /\ spaceShaded' = IF UseSpaceBarrier THEN TRUE ELSE spaceShaded
    /\ UNCHANGED <<ruleRoot, envRoot, ruleShaded, envShaded,
                  spaceMarked, ruleMarked, envMarked,
                  spaceFreed, ruleFreed, envFreed, phase>>

DeleteRuleRoot ==
    /\ phase = "marking"
    /\ ruleRoot
    /\ ruleRoot' = FALSE
    /\ ruleShaded' = IF UseRuleBarrier THEN TRUE ELSE ruleShaded
    /\ UNCHANGED <<spaceRoot, envRoot, spaceShaded, envShaded,
                  spaceMarked, ruleMarked, envMarked,
                  spaceFreed, ruleFreed, envFreed, phase>>

DeleteEnvRoot ==
    /\ phase = "marking"
    /\ envRoot
    /\ envRoot' = FALSE
    /\ envShaded' = IF UseEnvBarrier THEN TRUE ELSE envShaded
    /\ UNCHANGED <<spaceRoot, ruleRoot, spaceShaded, ruleShaded,
                  spaceMarked, ruleMarked, envMarked,
                  spaceFreed, ruleFreed, envFreed, phase>>

MarkSpaceRoot ==
    /\ phase = "marking"
    /\ ~spaceMarked
    /\ spaceRoot \/ spaceShaded
    /\ spaceMarked' = TRUE
    /\ UNCHANGED <<spaceRoot, ruleRoot, envRoot,
                  spaceShaded, ruleShaded, envShaded,
                  ruleMarked, envMarked,
                  spaceFreed, ruleFreed, envFreed, phase>>

MarkRuleRoot ==
    /\ phase = "marking"
    /\ ~ruleMarked
    /\ ruleRoot \/ ruleShaded
    /\ ruleMarked' = TRUE
    /\ UNCHANGED <<spaceRoot, ruleRoot, envRoot,
                  spaceShaded, ruleShaded, envShaded,
                  spaceMarked, envMarked,
                  spaceFreed, ruleFreed, envFreed, phase>>

MarkEnvRoot ==
    /\ phase = "marking"
    /\ ~envMarked
    /\ envRoot \/ envShaded
    /\ envMarked' = TRUE
    /\ UNCHANGED <<spaceRoot, ruleRoot, envRoot,
                  spaceShaded, ruleShaded, envShaded,
                  spaceMarked, ruleMarked,
                  spaceFreed, ruleFreed, envFreed, phase>>

MarkComplete ==
    /\ (spaceRoot \/ spaceShaded) => spaceMarked
    /\ (ruleRoot \/ ruleShaded) => ruleMarked
    /\ (envRoot \/ envShaded) => envMarked

Sweep ==
    /\ phase = "marking"
    /\ MarkComplete
    /\ phase' = "swept"
    /\ spaceFreed' = ~spaceMarked
    /\ ruleFreed' = ~ruleMarked
    /\ envFreed' = ~envMarked
    /\ UNCHANGED <<spaceRoot, ruleRoot, envRoot,
                  spaceShaded, ruleShaded, envShaded,
                  spaceMarked, ruleMarked, envMarked>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ DeleteSpaceRoot
    \/ DeleteRuleRoot
    \/ DeleteEnvRoot
    \/ MarkSpaceRoot
    \/ MarkRuleRoot
    \/ MarkEnvRoot
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoE0SnapshotLiveFreed ==
    /\ ~spaceFreed
    /\ ~ruleFreed
    /\ ~envFreed

=============================================================================
