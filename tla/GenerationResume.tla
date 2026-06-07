--------------------------- MODULE GenerationResume ---------------------------
(***************************************************************************)
(* E1/E5 generation-gated worker resume model.                            *)
(*                                                                        *)
(* UseGenerationResume = TRUE  => production: resume when gen # myGen.    *)
(* UseGenerationResume = FALSE => rejected boolean: resume when            *)
(*                                ~GC_REQUESTED.                          *)
(*                                                                        *)
(* BumpAtEnd = TRUE  => production: cycle end advances GC_CYCLE_GEN.      *)
(* BumpAtEnd = FALSE => buggy: the worker's generation never ends.        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS UseGenerationResume, BumpAtEnd

VARIABLES
    gen,
    myGen,
    gcRequested,
    ended,
    backToBackRequested,
    triedResume,
    resumed

vars == <<gen, myGen, gcRequested, ended, backToBackRequested, triedResume, resumed>>

TypeOK ==
    /\ gen \in 1..2
    /\ myGen \in 1..2
    /\ gcRequested \in BOOLEAN
    /\ ended \in BOOLEAN
    /\ backToBackRequested \in BOOLEAN
    /\ triedResume \in BOOLEAN
    /\ resumed \in BOOLEAN

Init ==
    /\ gen = 1
    /\ myGen = 1
    /\ gcRequested = TRUE
    /\ ended = FALSE
    /\ backToBackRequested = FALSE
    /\ triedResume = FALSE
    /\ resumed = FALSE

EndCycle ==
    /\ ~ended
    /\ ended' = TRUE
    /\ gen' = IF BumpAtEnd THEN 2 ELSE gen
    /\ gcRequested' = FALSE
    /\ UNCHANGED <<myGen, backToBackRequested, triedResume, resumed>>

BackToBackRequest ==
    /\ ended
    /\ ~backToBackRequested
    /\ gcRequested' = TRUE
    /\ backToBackRequested' = TRUE
    /\ UNCHANGED <<gen, myGen, ended, triedResume, resumed>>

CanResume ==
    IF UseGenerationResume
      THEN gen # myGen
      ELSE ~gcRequested

TryResume ==
    /\ ended
    /\ backToBackRequested
    /\ ~triedResume
    /\ triedResume' = TRUE
    /\ resumed' = CanResume
    /\ UNCHANGED <<gen, myGen, gcRequested, ended, backToBackRequested>>

Done ==
    /\ UNCHANGED vars

Next ==
    \/ EndCycle
    \/ BackToBackRequest
    \/ TryResume
    \/ Done

Spec == Init /\ [][Next]_vars

EndedCycleCanResume ==
    ~(triedResume /\ ~resumed)

=============================================================================
