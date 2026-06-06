--------------------------- MODULE TieredCacheBarriers ---------------------------
(***************************************************************************)
(* Global tiered-cache SATB/rooting discriminator.                          *)
(*                                                                         *)
(* The tiered cache contributes pending bytecode source roots and compiled  *)
(* bytecode constants as E0 structural roots. Removed pending roots and     *)
(* cleared compiled constants must be shaded during an active SATB mark.    *)
(* Pending root guards are token-checked so an old guard cannot unregister  *)
(* a newer root registered for the same expression hash.                    *)
(***************************************************************************)

CONSTANTS
    ScanPending,
    ScanCompiled,
    ShadeOverwrite,
    ShadeCancel,
    ShadeGuardDrop,
    ShadeClearPending,
    ShadeClearCompiled,
    TokenCheckedDrop

VARIABLES
    phase,
    pendingRegistered,
    compiledRegistered,
    pendingScanned,
    compiledScanned,
    oldOverwritten,
    oldCancelled,
    oldGuardDropped,
    oldClearedPending,
    oldClearedCompiled,
    shadedOverwrite,
    shadedCancel,
    shadedGuardDrop,
    shadedClearPending,
    shadedClearCompiled,
    newerWronglyRemoved,
    freed

vars ==
    <<phase, pendingRegistered, compiledRegistered, pendingScanned, compiledScanned,
      oldOverwritten, oldCancelled, oldGuardDropped, oldClearedPending, oldClearedCompiled,
      shadedOverwrite, shadedCancel, shadedGuardDrop, shadedClearPending, shadedClearCompiled,
      newerWronglyRemoved, freed>>

TypeOK ==
    /\ phase \in {"start", "pending", "overwritten", "guard_dropped", "compiled",
                  "cancelled", "cleared", "scanned", "swept", "done"}
    /\ pendingRegistered \in BOOLEAN
    /\ compiledRegistered \in BOOLEAN
    /\ pendingScanned \in BOOLEAN
    /\ compiledScanned \in BOOLEAN
    /\ oldOverwritten \in BOOLEAN
    /\ oldCancelled \in BOOLEAN
    /\ oldGuardDropped \in BOOLEAN
    /\ oldClearedPending \in BOOLEAN
    /\ oldClearedCompiled \in BOOLEAN
    /\ shadedOverwrite \in BOOLEAN
    /\ shadedCancel \in BOOLEAN
    /\ shadedGuardDrop \in BOOLEAN
    /\ shadedClearPending \in BOOLEAN
    /\ shadedClearCompiled \in BOOLEAN
    /\ newerWronglyRemoved \in BOOLEAN
    /\ freed \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ pendingRegistered = FALSE
    /\ compiledRegistered = FALSE
    /\ pendingScanned = FALSE
    /\ compiledScanned = FALSE
    /\ oldOverwritten = FALSE
    /\ oldCancelled = FALSE
    /\ oldGuardDropped = FALSE
    /\ oldClearedPending = FALSE
    /\ oldClearedCompiled = FALSE
    /\ shadedOverwrite = FALSE
    /\ shadedCancel = FALSE
    /\ shadedGuardDrop = FALSE
    /\ shadedClearPending = FALSE
    /\ shadedClearCompiled = FALSE
    /\ newerWronglyRemoved = FALSE
    /\ freed = FALSE

RegisterPending ==
    /\ phase = "start"
    /\ phase' = "pending"
    /\ pendingRegistered' = TRUE
    /\ UNCHANGED <<compiledRegistered, pendingScanned, compiledScanned,
                  oldOverwritten, oldCancelled, oldGuardDropped,
                  oldClearedPending, oldClearedCompiled, shadedOverwrite,
                  shadedCancel, shadedGuardDrop, shadedClearPending,
                  shadedClearCompiled, newerWronglyRemoved, freed>>

OverwritePending ==
    /\ phase = "pending"
    /\ phase' = "overwritten"
    /\ oldOverwritten' = TRUE
    /\ shadedOverwrite' = ShadeOverwrite
    /\ UNCHANGED <<pendingRegistered, compiledRegistered, pendingScanned, compiledScanned,
                  oldCancelled, oldGuardDropped, oldClearedPending, oldClearedCompiled,
                  shadedCancel, shadedGuardDrop, shadedClearPending, shadedClearCompiled,
                  newerWronglyRemoved, freed>>

OldGuardDrop ==
    /\ phase = "overwritten"
    /\ phase' = "guard_dropped"
    /\ oldGuardDropped' = TRUE
    /\ shadedGuardDrop' = ShadeGuardDrop
    /\ pendingRegistered' = IF TokenCheckedDrop THEN pendingRegistered ELSE FALSE
    /\ newerWronglyRemoved' = IF TokenCheckedDrop THEN newerWronglyRemoved ELSE TRUE
    /\ UNCHANGED <<compiledRegistered, pendingScanned, compiledScanned,
                  oldOverwritten, oldCancelled, oldClearedPending, oldClearedCompiled,
                  shadedOverwrite, shadedCancel, shadedClearPending, shadedClearCompiled,
                  freed>>

RegisterCompiled ==
    /\ phase = "guard_dropped"
    /\ phase' = "compiled"
    /\ compiledRegistered' = TRUE
    /\ UNCHANGED <<pendingRegistered, pendingScanned, compiledScanned,
                  oldOverwritten, oldCancelled, oldGuardDropped,
                  oldClearedPending, oldClearedCompiled, shadedOverwrite,
                  shadedCancel, shadedGuardDrop, shadedClearPending,
                  shadedClearCompiled, newerWronglyRemoved, freed>>

CancelPending ==
    /\ phase = "compiled"
    /\ phase' = "cancelled"
    /\ oldCancelled' = TRUE
    /\ shadedCancel' = ShadeCancel
    /\ UNCHANGED <<pendingRegistered, compiledRegistered, pendingScanned, compiledScanned,
                  oldOverwritten, oldGuardDropped, oldClearedPending, oldClearedCompiled,
                  shadedOverwrite, shadedGuardDrop, shadedClearPending, shadedClearCompiled,
                  newerWronglyRemoved, freed>>

ClearCache ==
    /\ phase = "cancelled"
    /\ phase' = "cleared"
    /\ oldClearedPending' = TRUE
    /\ oldClearedCompiled' = TRUE
    /\ shadedClearPending' = ShadeClearPending
    /\ shadedClearCompiled' = ShadeClearCompiled
    /\ UNCHANGED <<pendingRegistered, compiledRegistered, pendingScanned, compiledScanned,
                  oldOverwritten, oldCancelled, oldGuardDropped, shadedOverwrite,
                  shadedCancel, shadedGuardDrop, newerWronglyRemoved, freed>>

ScanRoots ==
    /\ phase = "cleared"
    /\ phase' = "scanned"
    /\ pendingScanned' = (pendingRegistered /\ ScanPending)
    /\ compiledScanned' = (compiledRegistered /\ ScanCompiled)
    /\ UNCHANGED <<pendingRegistered, compiledRegistered, oldOverwritten, oldCancelled,
                  oldGuardDropped, oldClearedPending, oldClearedCompiled,
                  shadedOverwrite, shadedCancel, shadedGuardDrop,
                  shadedClearPending, shadedClearCompiled, newerWronglyRemoved, freed>>

Sweep ==
    /\ phase = "scanned"
    /\ phase' = "swept"
    /\ freed' =
        ((pendingRegistered /\ ~pendingScanned) \/
         (compiledRegistered /\ ~compiledScanned) \/
         (oldOverwritten /\ ~shadedOverwrite) \/
         (oldCancelled /\ ~shadedCancel) \/
         (oldGuardDropped /\ ~shadedGuardDrop) \/
         (oldClearedPending /\ ~shadedClearPending) \/
         (oldClearedCompiled /\ ~shadedClearCompiled) \/
         newerWronglyRemoved)
    /\ UNCHANGED <<pendingRegistered, compiledRegistered, pendingScanned, compiledScanned,
                  oldOverwritten, oldCancelled, oldGuardDropped,
                  oldClearedPending, oldClearedCompiled, shadedOverwrite,
                  shadedCancel, shadedGuardDrop, shadedClearPending,
                  shadedClearCompiled, newerWronglyRemoved>>

Finish ==
    /\ phase = "swept"
    /\ phase' = "done"
    /\ UNCHANGED <<pendingRegistered, compiledRegistered, pendingScanned, compiledScanned,
                  oldOverwritten, oldCancelled, oldGuardDropped,
                  oldClearedPending, oldClearedCompiled, shadedOverwrite,
                  shadedCancel, shadedGuardDrop, shadedClearPending,
                  shadedClearCompiled, newerWronglyRemoved, freed>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ RegisterPending
    \/ OverwritePending
    \/ OldGuardDrop
    \/ RegisterCompiled
    \/ CancelPending
    \/ ClearCache
    \/ ScanRoots
    \/ Sweep
    \/ Finish
    \/ Done

Spec == Init /\ [][Next]_vars

NoTieredCacheValueFreed ==
    ~freed

=============================================================================
