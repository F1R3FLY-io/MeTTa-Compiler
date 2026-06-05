-------------------------- MODULE DriverCPublication --------------------------
(***************************************************************************)
(* Driver-C publication model for the CESK index collector.                 *)
(*                                                                         *)
(* The driver owns source/output roots outside the active CESK machine.     *)
(* During an eval transition, midloop/rendezvous GC can only see them if    *)
(* the eval boundary publishes them to SAFEPOINT_ROOTS. The positive config *)
(* publishes at eval entry. The negative config omits that publication and  *)
(* must violate DriverCVisibleOnSweep.                                      *)
(***************************************************************************)

CONSTANTS
    PublishAtEvalEntry

VARIABLES
    evalLive,
    driverCLive,
    safepointHasDriverC,
    rootSetBuilt,
    rootSetHasDriverC,
    swept

vars ==
    <<evalLive, driverCLive, safepointHasDriverC,
      rootSetBuilt, rootSetHasDriverC, swept>>

TypeOK ==
    /\ evalLive \in BOOLEAN
    /\ driverCLive \in BOOLEAN
    /\ safepointHasDriverC \in BOOLEAN
    /\ rootSetBuilt \in BOOLEAN
    /\ rootSetHasDriverC \in BOOLEAN
    /\ swept \in BOOLEAN

Init ==
    /\ evalLive = FALSE
    /\ driverCLive = TRUE
    /\ safepointHasDriverC = FALSE
    /\ rootSetBuilt = FALSE
    /\ rootSetHasDriverC = FALSE
    /\ swept = FALSE

EnterEval ==
    /\ ~evalLive
    /\ ~swept
    /\ evalLive' = TRUE
    /\ safepointHasDriverC' = PublishAtEvalEntry
    /\ UNCHANGED <<driverCLive, rootSetBuilt, rootSetHasDriverC, swept>>

BuildMidloopRoots ==
    /\ evalLive
    /\ ~rootSetBuilt
    /\ ~swept
    /\ rootSetBuilt' = TRUE
    /\ rootSetHasDriverC' = safepointHasDriverC
    /\ UNCHANGED <<evalLive, driverCLive, safepointHasDriverC, swept>>

Sweep ==
    /\ evalLive
    /\ rootSetBuilt
    /\ ~swept
    /\ swept' = TRUE
    /\ UNCHANGED <<evalLive, driverCLive, safepointHasDriverC,
                  rootSetBuilt, rootSetHasDriverC>>

Done ==
    /\ swept
    /\ UNCHANGED vars

Next ==
    \/ EnterEval
    \/ BuildMidloopRoots
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

DriverCVisibleOnSweep ==
    swept => (driverCLive => rootSetHasDriverC)

=============================================================================
