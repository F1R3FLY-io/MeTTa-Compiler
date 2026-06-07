------------------- MODULE RendezvousQuiescenceIndependence -------------------
(***************************************************************************)
(* Dedicated CESK rendezvous collection is gated by the per-participant      *)
(* witness/root-contribution protocol, not by global active-evaluator        *)
(* quiescence. The negative config reintroduces the old active==0 gate and   *)
(* must fail once a nonquiescent rendezvous is ready to sweep.               *)
(***************************************************************************)

EXTENDS Naturals

CONSTANTS
    ActiveWorkers,
    UseQuiescenceGate

VARIABLES
    phase,
    active,
    contributed,
    witnessOk,
    swept

vars ==
    <<phase, active, contributed, witnessOk, swept>>

Phases == {"init", "contributed", "ready", "swept"}

TypeOK ==
    /\ ActiveWorkers \in Nat
    /\ UseQuiescenceGate \in BOOLEAN
    /\ phase \in Phases
    /\ active \in Nat
    /\ contributed \in BOOLEAN
    /\ witnessOk \in BOOLEAN
    /\ swept \in BOOLEAN

Init ==
    /\ phase = "init"
    /\ active = ActiveWorkers
    /\ contributed = FALSE
    /\ witnessOk = FALSE
    /\ swept = FALSE

Contribute ==
    /\ phase = "init"
    /\ contributed' = TRUE
    /\ phase' = "contributed"
    /\ UNCHANGED <<active, witnessOk, swept>>

SetWitness ==
    /\ phase = "contributed"
    /\ contributed
    /\ witnessOk' = TRUE
    /\ phase' = "ready"
    /\ UNCHANGED <<active, contributed, swept>>

SweepEnabled ==
    /\ contributed
    /\ witnessOk
    /\ (UseQuiescenceGate => active = 0)

Sweep ==
    /\ phase = "ready"
    /\ SweepEnabled
    /\ swept' = TRUE
    /\ phase' = "swept"
    /\ UNCHANGED <<active, contributed, witnessOk>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ Contribute
    \/ SetWitness
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

ReadyCanSweep ==
    phase = "ready" => SweepEnabled

SweepNeedsWitness ==
    swept => contributed /\ witnessOk

=============================================================================
