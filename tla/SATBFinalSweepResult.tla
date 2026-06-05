--------------------------- MODULE SATBFinalSweepResult ---------------------------
(***************************************************************************)
(* E2 SATB final-sweep result discriminator.                               *)
(*                                                                         *)
(* The final rendezvous sweep is guarded by the same witness gate as the   *)
(* normal rendezvous collector. If that gate unexpectedly closes, the Rust *)
(* helper returns false. The driver must observe that false result and run  *)
(* the abort-to-STW backstop; otherwise it can drop the final roots and     *)
(* finish the request without either sweeping or falling back.              *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    CheckSweepResult,
    FinalGateOpen

VARIABLES
    phase,
    swept,
    fallbackRan

vars == <<phase, swept, fallbackRan>>

TypeOK ==
    /\ phase \in {"final", "sweep_returned", "fallback", "done"}
    /\ swept \in BOOLEAN
    /\ fallbackRan \in BOOLEAN

Init ==
    /\ phase = "final"
    /\ swept = FALSE
    /\ fallbackRan = FALSE

TryFinalSweep ==
    /\ phase = "final"
    /\ phase' = "sweep_returned"
    /\ swept' = FinalGateOpen
    /\ UNCHANGED fallbackRan

CheckClosedGate ==
    /\ phase = "sweep_returned"
    /\ ~swept
    /\ CheckSweepResult
    /\ phase' = "fallback"
    /\ fallbackRan' = TRUE
    /\ UNCHANGED swept

IgnoreClosedGate ==
    /\ phase = "sweep_returned"
    /\ ~swept
    /\ ~CheckSweepResult
    /\ phase' = "done"
    /\ UNCHANGED <<swept, fallbackRan>>

FinishSwept ==
    /\ phase = "sweep_returned"
    /\ swept
    /\ phase' = "done"
    /\ UNCHANGED <<swept, fallbackRan>>

FinishFallback ==
    /\ phase = "fallback"
    /\ phase' = "done"
    /\ UNCHANGED <<swept, fallbackRan>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ TryFinalSweep
    \/ CheckClosedGate
    \/ IgnoreClosedGate
    \/ FinishSwept
    \/ FinishFallback
    \/ Done

Spec == Init /\ [][Next]_vars

FinalSweepHandled ==
    phase = "done" => swept \/ fallbackRan

=============================================================================
