---------------------------- MODULE DriverRootUnion ----------------------------
(***************************************************************************)
(* E1 driver root-union channel model.                                     *)
(*                                                                         *)
(* The rendezvous driver root set is the union of four source channels:    *)
(*   - worker root buffer: parked participant CESK machines                *)
(*   - safepoint roots: driver-C / leaving result roots                    *)
(*   - live env anchors: persistent E0 roots                               *)
(*   - live dispatch anchors: parallel fan-out inputs/outputs              *)
(*                                                                         *)
(* A sweep is safe only if every live channel root is included. The        *)
(* positive config includes all channels. Negative configs omit one        *)
(* load-bearing channel and must violate RootUnionComplete.                *)
(***************************************************************************)

CONSTANTS
    IncludeWorker,
    IncludeSafepoint,
    IncludeEnv,
    IncludeDispatch

VARIABLES
    workerLive,
    safepointLive,
    envLive,
    dispatchLive,
    workerRooted,
    safepointRooted,
    envRooted,
    dispatchRooted,
    built,
    swept

vars ==
    <<workerLive, safepointLive, envLive, dispatchLive,
      workerRooted, safepointRooted, envRooted, dispatchRooted, built, swept>>

TypeOK ==
    /\ workerLive \in BOOLEAN
    /\ safepointLive \in BOOLEAN
    /\ envLive \in BOOLEAN
    /\ dispatchLive \in BOOLEAN
    /\ workerRooted \in BOOLEAN
    /\ safepointRooted \in BOOLEAN
    /\ envRooted \in BOOLEAN
    /\ dispatchRooted \in BOOLEAN
    /\ built \in BOOLEAN
    /\ swept \in BOOLEAN

Init ==
    /\ workerLive = TRUE
    /\ safepointLive = TRUE
    /\ envLive = TRUE
    /\ dispatchLive = TRUE
    /\ workerRooted = FALSE
    /\ safepointRooted = FALSE
    /\ envRooted = FALSE
    /\ dispatchRooted = FALSE
    /\ built = FALSE
    /\ swept = FALSE

BuildRootUnion ==
    /\ ~swept
    /\ ~built
    /\ workerRooted' = IncludeWorker /\ workerLive
    /\ safepointRooted' = IncludeSafepoint /\ safepointLive
    /\ envRooted' = IncludeEnv /\ envLive
    /\ dispatchRooted' = IncludeDispatch /\ dispatchLive
    /\ built' = TRUE
    /\ UNCHANGED <<workerLive, safepointLive, envLive, dispatchLive, swept>>

Sweep ==
    /\ ~swept
    /\ built
    /\ swept' = TRUE
    /\ UNCHANGED <<workerLive, safepointLive, envLive, dispatchLive,
                  workerRooted, safepointRooted, envRooted, dispatchRooted, built>>

Done ==
    /\ swept
    /\ UNCHANGED vars

Next ==
    \/ BuildRootUnion
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

RootUnionComplete ==
    swept =>
      /\ workerLive => workerRooted
      /\ safepointLive => safepointRooted
      /\ envLive => envRooted
      /\ dispatchLive => dispatchRooted

=============================================================================
