-------------------------- MODULE SATBDeletionBarrier --------------------------
(***************************************************************************)
(* E2 SATB Yuasa deletion-barrier discriminator.                          *)
(*                                                                        *)
(* A value reachable at snapshot time may be deleted before the marker     *)
(* reaches it. With a SATB deletion barrier, the removed pre-image is      *)
(* shaded and must be marked before sweep. Without the barrier, the mark   *)
(* completion predicate becomes vacuously true after deletion and sweep can *)
(* free a snapshot-live value.                                             *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT UseBarrier

VARIABLES
    root,
    shaded,
    marked,
    freed,
    phase

vars == <<root, shaded, marked, freed, phase>>

TypeOK ==
    /\ root \in BOOLEAN
    /\ shaded \in BOOLEAN
    /\ marked \in BOOLEAN
    /\ freed \in BOOLEAN
    /\ phase \in {"marking", "swept"}

Init ==
    /\ root = TRUE
    /\ shaded = FALSE
    /\ marked = FALSE
    /\ freed = FALSE
    /\ phase = "marking"

DeleteRoot ==
    /\ phase = "marking"
    /\ root
    /\ root' = FALSE
    /\ shaded' = IF UseBarrier THEN TRUE ELSE shaded
    /\ UNCHANGED <<marked, freed, phase>>

Mark ==
    /\ phase = "marking"
    /\ ~marked
    /\ (root \/ shaded)
    /\ marked' = TRUE
    /\ UNCHANGED <<root, shaded, freed, phase>>

MarkComplete ==
    (root \/ shaded) => marked

Sweep ==
    /\ phase = "marking"
    /\ MarkComplete
    /\ phase' = "swept"
    /\ freed' = ~marked
    /\ UNCHANGED <<root, shaded, marked>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ DeleteRoot
    \/ Mark
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoSnapshotLiveFreed ==
    ~freed

=============================================================================
