-------------------------- MODULE WitnessSlotLifecycle --------------------------
(***************************************************************************)
(* V4 witness slot lifecycle model.                                         *)
(*                                                                         *)
(* Load-bearing source invariant: the witness slot stays occupied from     *)
(* EvalGuard outermost enter until the true outermost EvalGuard drop,       *)
(* including across safepoint drops / parks. A safepoint drop removes the  *)
(* thread from N_THREADS but must NOT un-occupy the slot, because the       *)
(* frozen machine is still live and must either block sweep or be buffered *)
(* by a genuine park.                                                       *)
(*                                                                         *)
(* ReleaseOnSafepoint = FALSE models V4. ReleaseOnSafepoint = TRUE models  *)
(* the pre-V4 bug: a parked live machine becomes unoccupied, the driver     *)
(* skips it, and a sweep can run with no buffered root.                     *)
(***************************************************************************)

CONSTANT ReleaseOnSafepoint

VARIABLES
    live,
    occupied,
    buffered,
    swept

vars == <<live, occupied, buffered, swept>>

TypeOK ==
    /\ live \in BOOLEAN
    /\ occupied \in BOOLEAN
    /\ buffered \in BOOLEAN
    /\ swept \in BOOLEAN

Init ==
    /\ live = FALSE
    /\ occupied = FALSE
    /\ buffered = FALSE
    /\ swept = FALSE

Enter ==
    /\ ~swept
    /\ ~live
    /\ live' = TRUE
    /\ occupied' = TRUE
    /\ buffered' = FALSE
    /\ UNCHANGED swept

SafepointDrop ==
    /\ ~swept
    /\ live
    /\ occupied' = IF ReleaseOnSafepoint THEN FALSE ELSE occupied
    /\ UNCHANGED <<live, buffered, swept>>

PublishPark ==
    /\ ~swept
    /\ live
    /\ occupied
    /\ buffered' = TRUE
    /\ UNCHANGED <<live, occupied, swept>>

OutermostDrop ==
    /\ ~swept
    /\ live
    /\ live' = FALSE
    /\ occupied' = FALSE
    /\ buffered' = FALSE
    /\ UNCHANGED swept

CanSweep ==
    ~occupied \/ buffered

Sweep ==
    /\ ~swept
    /\ CanSweep
    /\ swept' = TRUE
    /\ UNCHANGED <<live, occupied, buffered>>

Done ==
    /\ swept
    /\ UNCHANGED vars

Next ==
    \/ Enter
    \/ SafepointDrop
    \/ PublishPark
    \/ OutermostDrop
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

LiveMachineVisibleOnSweep ==
    swept => (live => buffered)

=============================================================================
