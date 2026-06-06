---------------------------- MODULE WriteOnceAnchors ----------------------------
(***************************************************************************)
(* Write-once compiler-atom anchor discriminator.                           *)
(*                                                                         *)
(* The compiler currently has three OnceLock<MettaValue> anchors: "=",      *)
(* "println!", and "if".  The positive config scans all initialized anchors *)
(* and has no deletion transition.  Negative configs either omit one anchor *)
(* from the structural reader or allow a reset/take-style deletion.         *)
(***************************************************************************)

CONSTANTS
    ScanEquals,
    ScanPrintln,
    ScanIf,
    AllowDelete

VARIABLES
    phase,
    equalsInit,
    printlnInit,
    ifInit,
    equalsDeleted,
    printlnDeleted,
    ifDeleted,
    equalsScanned,
    printlnScanned,
    ifScanned

vars ==
    <<phase, equalsInit, printlnInit, ifInit,
      equalsDeleted, printlnDeleted, ifDeleted,
      equalsScanned, printlnScanned, ifScanned>>

TypeOK ==
    /\ phase \in {"start", "initialized", "deleted", "scanned", "done"}
    /\ equalsInit \in BOOLEAN
    /\ printlnInit \in BOOLEAN
    /\ ifInit \in BOOLEAN
    /\ equalsDeleted \in BOOLEAN
    /\ printlnDeleted \in BOOLEAN
    /\ ifDeleted \in BOOLEAN
    /\ equalsScanned \in BOOLEAN
    /\ printlnScanned \in BOOLEAN
    /\ ifScanned \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ equalsInit = FALSE
    /\ printlnInit = FALSE
    /\ ifInit = FALSE
    /\ equalsDeleted = FALSE
    /\ printlnDeleted = FALSE
    /\ ifDeleted = FALSE
    /\ equalsScanned = FALSE
    /\ printlnScanned = FALSE
    /\ ifScanned = FALSE

InitializeAnchors ==
    /\ phase = "start"
    /\ phase' = "initialized"
    /\ equalsInit' = TRUE
    /\ printlnInit' = TRUE
    /\ ifInit' = TRUE
    /\ UNCHANGED <<equalsDeleted, printlnDeleted, ifDeleted,
                  equalsScanned, printlnScanned, ifScanned>>

DeleteIfAnchor ==
    /\ phase = "initialized"
    /\ AllowDelete
    /\ phase' = "deleted"
    /\ ifDeleted' = TRUE
    /\ UNCHANGED <<equalsInit, printlnInit, ifInit,
                  equalsDeleted, printlnDeleted,
                  equalsScanned, printlnScanned, ifScanned>>

ScanAnchors ==
    /\ phase \in {"initialized", "deleted"}
    /\ phase' = "scanned"
    /\ equalsScanned' = (ScanEquals /\ equalsInit /\ ~equalsDeleted)
    /\ printlnScanned' = (ScanPrintln /\ printlnInit /\ ~printlnDeleted)
    /\ ifScanned' = (ScanIf /\ ifInit /\ ~ifDeleted)
    /\ UNCHANGED <<equalsInit, printlnInit, ifInit,
                  equalsDeleted, printlnDeleted, ifDeleted>>

Finish ==
    /\ phase = "scanned"
    /\ phase' = "done"
    /\ UNCHANGED <<equalsInit, printlnInit, ifInit,
                  equalsDeleted, printlnDeleted, ifDeleted,
                  equalsScanned, printlnScanned, ifScanned>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ InitializeAnchors
    \/ DeleteIfAnchor
    \/ ScanAnchors
    \/ Finish
    \/ Done

Spec == Init /\ [][Next]_vars

NoAnchorDeleted ==
    ~(equalsDeleted \/ printlnDeleted \/ ifDeleted)

LiveAnchorsScanned ==
    phase \in {"scanned", "done"} =>
        /\ ((equalsInit /\ ~equalsDeleted) => equalsScanned)
        /\ ((printlnInit /\ ~printlnDeleted) => printlnScanned)
        /\ ((ifInit /\ ~ifDeleted) => ifScanned)

=============================================================================
