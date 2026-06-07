-------------------------- MODULE MajorMinorScheduler --------------------------
EXTENDS Integers
(***************************************************************************)
(* C1.c major/minor scheduler discriminator.                              *)
(*                                                                        *)
(* The implementation may choose a minor instead of a coincident           *)
(* live-growth major only under acute level-3 young pressure. Hard-cap and *)
(* cadence majors are not deferrable.                                     *)
(*                                                                        *)
(* RequireLevel3=FALSE   => a live major can defer below level 3.          *)
(* ProtectCap=FALSE      => a cap-forced major can defer.                  *)
(* ProtectCadence=FALSE  => a cadence-forced major can defer.              *)
(***************************************************************************)

CONSTANTS RequireLevel3, ProtectCap, ProtectCadence

VARIABLES
    phase,
    youngPressure,
    nurseryPending,
    liveMajor,
    capMajor,
    cadenceMajor,
    doMajor

vars == <<phase, youngPressure, nurseryPending, liveMajor, capMajor, cadenceMajor, doMajor>>

TypeOK ==
    /\ RequireLevel3 \in BOOLEAN
    /\ ProtectCap \in BOOLEAN
    /\ ProtectCadence \in BOOLEAN
    /\ phase \in {"start", "inputs", "chosen"}
    /\ youngPressure \in 0..3
    /\ nurseryPending \in BOOLEAN
    /\ liveMajor \in BOOLEAN
    /\ capMajor \in BOOLEAN
    /\ cadenceMajor \in BOOLEAN
    /\ doMajor \in BOOLEAN

MajorDue == liveMajor \/ capMajor \/ cadenceMajor

MinorDue == youngPressure > 0 \/ nurseryPending

Level3 == youngPressure = 3

DeferAllowed ==
    /\ IF RequireLevel3 THEN Level3 ELSE TRUE
    /\ MinorDue
    /\ IF ProtectCap THEN ~capMajor ELSE TRUE
    /\ IF ProtectCadence THEN ~cadenceMajor ELSE TRUE

Init ==
    /\ phase = "start"
    /\ youngPressure = 0
    /\ nurseryPending = FALSE
    /\ liveMajor = FALSE
    /\ capMajor = FALSE
    /\ cadenceMajor = FALSE
    /\ doMajor = FALSE

PickInputs ==
    /\ phase = "start"
    /\ youngPressure' \in 0..3
    /\ nurseryPending' \in BOOLEAN
    /\ liveMajor' \in BOOLEAN
    /\ capMajor' \in BOOLEAN
    /\ cadenceMajor' \in BOOLEAN
    /\ phase' = "inputs"
    /\ UNCHANGED doMajor

Choose ==
    /\ phase = "inputs"
    /\ doMajor' = (MajorDue /\ ~DeferAllowed)
    /\ phase' = "chosen"
    /\ UNCHANGED <<youngPressure, nurseryPending, liveMajor, capMajor, cadenceMajor>>

Done ==
    /\ phase = "chosen"
    /\ UNCHANGED vars

Next ==
    \/ PickInputs
    \/ Choose
    \/ Done

Spec == Init /\ [][Next]_vars

CapMajorNotDeferred ==
    ~(phase = "chosen" /\ capMajor /\ ~doMajor)

CadenceMajorNotDeferred ==
    ~(phase = "chosen" /\ cadenceMajor /\ ~doMajor)

DeferredMajorRequiresLevel3 ==
    ~(phase = "chosen" /\ MajorDue /\ ~doMajor /\ ~Level3)

DeferredMajorIsLiveOnly ==
    ~(phase = "chosen" /\ MajorDue /\ ~doMajor /\ ~liveMajor)

NoMajorWhenNoMajorDue ==
    ~(phase = "chosen" /\ ~MajorDue /\ doMajor)

=============================================================================
