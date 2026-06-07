-------------------------- MODULE CapFloorAntiThrash --------------------------
EXTENDS Integers
(***************************************************************************)
(* B.5 cap-floor anti-thrash model.                                      *)
(*                                                                        *)
(* A cap-triggered major that releases no segment raises CAP_FLOOR to the *)
(* current committed size. With unchanged committed bytes, the cap clause  *)
(* cannot immediately re-fire. A segment-releasing major clears the floor  *)
(* so the ordinary base cap is restored.                                  *)
(*                                                                        *)
(* RaiseOnFutile=FALSE => futile cap major immediately re-fires.          *)
(* ClearOnRelease=FALSE => stale floor survives a segment-releasing major. *)
(***************************************************************************)

CONSTANTS RaiseOnFutile, ClearOnRelease

VARIABLES
    phase,
    committed,
    baseCap,
    oldFloor,
    released,
    capBefore,
    nextFloor,
    capAfter

vars == <<phase, committed, baseCap, oldFloor, released,
          capBefore, nextFloor, capAfter>>

Max(a, b) == IF a >= b THEN a ELSE b

TypeOK ==
    /\ RaiseOnFutile \in BOOLEAN
    /\ ClearOnRelease \in BOOLEAN
    /\ phase \in {"start", "inputs", "checked"}
    /\ committed \in 0..4
    /\ baseCap \in 0..3
    /\ oldFloor \in 0..4
    /\ released \in 0..1
    /\ capBefore \in BOOLEAN
    /\ nextFloor \in 0..4
    /\ capAfter \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ committed = 0
    /\ baseCap = 0
    /\ oldFloor = 0
    /\ released = 0
    /\ capBefore = FALSE
    /\ nextFloor = 0
    /\ capAfter = FALSE

PickInputs ==
    /\ phase = "start"
    /\ committed' \in 0..4
    /\ baseCap' \in 0..3
    /\ oldFloor' \in 0..4
    /\ released' \in 0..1
    /\ capBefore' = (committed' > Max(baseCap', oldFloor'))
    /\ phase' = "inputs"
    /\ UNCHANGED <<nextFloor, capAfter>>

UpdateFloorAndCheck ==
    /\ phase = "inputs"
    /\ nextFloor' =
        IF capBefore /\ released = 0 /\ RaiseOnFutile THEN committed
        ELSE IF released > 0 /\ ClearOnRelease THEN 0
        ELSE oldFloor
    /\ capAfter' = (committed > Max(baseCap, nextFloor'))
    /\ phase' = "checked"
    /\ UNCHANGED <<committed, baseCap, oldFloor, released, capBefore>>

Done ==
    /\ phase = "checked"
    /\ UNCHANGED vars

Next ==
    \/ PickInputs
    \/ UpdateFloorAndCheck
    \/ Done

Spec == Init /\ [][Next]_vars

FutileCapMajorDoesNotRefire ==
    ~(phase = "checked" /\ capBefore /\ released = 0 /\ capAfter)

ReleaseClearsFloor ==
    ~(phase = "checked" /\ released > 0 /\ nextFloor # 0)

ReleaseRestoresBaseCap ==
    ~(phase = "checked" /\ released > 0 /\ committed > baseCap /\ ~capAfter)

NoCapWhenCommittedAtOrBelowEffectiveCap ==
    ~(phase = "checked" /\ committed <= Max(baseCap, nextFloor) /\ capAfter)

=============================================================================
