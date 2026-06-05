---------------------------- MODULE WitnessOkReset ----------------------------
(***************************************************************************)
(* E1/E5 cross-cycle witness flag reset model.                            *)
(*                                                                        *)
(* CURRENT_WITNESS_OK is a non-generational bool read by the rendezvous    *)
(* sweep gate. Therefore a cycle-end teardown must clear it after bumping  *)
(* GC_CYCLE_GEN and before the next cycle can collect. Otherwise the next  *)
(* cycle may observe the previous cycle's true flag and sweep before its   *)
(* own witness wait has proved root completeness.                          *)
(*                                                                        *)
(* ClearAtEnd = TRUE  => production teardown clears CURRENT_WITNESS_OK.    *)
(* ClearAtEnd = FALSE => stale true flag survives into the next cycle.     *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT ClearAtEnd

VARIABLES
    gen,
    proved,
    ok,
    collected1,
    stale

vars == <<gen, proved, ok, collected1, stale>>

TypeOK ==
    /\ gen \in 1..2
    /\ proved \in 0..2
    /\ proved <= gen
    /\ ok \in BOOLEAN
    /\ collected1 \in BOOLEAN
    /\ stale \in BOOLEAN

Init ==
    /\ gen = 1
    /\ proved = 0
    /\ ok = FALSE
    /\ collected1 = FALSE
    /\ stale = FALSE

ProveCycle1 ==
    /\ gen = 1
    /\ proved = 0
    /\ proved' = 1
    /\ ok' = TRUE
    /\ UNCHANGED <<gen, collected1, stale>>

Collect ==
    /\ ok
    /\ IF gen = 1 THEN ~collected1 ELSE TRUE
    /\ collected1' = IF gen = 1 THEN TRUE ELSE collected1
    /\ stale' = (stale \/ (proved < gen))
    /\ UNCHANGED <<gen, proved, ok>>

EndCycle ==
    /\ gen = 1
    /\ collected1
    /\ gen' = 2
    /\ ok' = IF ClearAtEnd THEN FALSE ELSE ok
    /\ UNCHANGED <<proved, collected1, stale>>

ProveCycle2 ==
    /\ gen = 2
    /\ proved = 1
    /\ proved' = 2
    /\ ok' = TRUE
    /\ UNCHANGED <<gen, collected1, stale>>

Done ==
    /\ UNCHANGED vars

Next ==
    \/ ProveCycle1
    \/ Collect
    \/ EndCycle
    \/ ProveCycle2
    \/ Done

Spec == Init /\ [][Next]_vars

NoStaleWitnessCollect ==
    ~stale

=============================================================================
