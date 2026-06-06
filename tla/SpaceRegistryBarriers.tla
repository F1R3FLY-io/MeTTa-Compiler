-------------------------- MODULE SpaceRegistryBarriers --------------------------
(***************************************************************************)
(* Global space-registry SATB/rooting discriminator.                        *)
(*                                                                         *)
(* Registered spaces are E0 global anchors and must be scanned by           *)
(* collect_global_anchors.  Replaced, removed, and bulk-cleared old         *)
(* SpaceHandles must have their contained values shaded during an active    *)
(* SATB mark before sweep is allowed to free unmarked values.               *)
(***************************************************************************)

CONSTANTS
    ScanRegistered,
    ShadeOverwrite,
    ShadeRemove,
    ShadeClear

VARIABLES
    phase,
    registered,
    scanned,
    oldOverwritten,
    oldRemoved,
    oldCleared,
    shadedOverwrite,
    shadedRemove,
    shadedClear,
    freed

vars ==
    <<phase, registered, scanned, oldOverwritten, oldRemoved, oldCleared,
      shadedOverwrite, shadedRemove, shadedClear, freed>>

TypeOK ==
    /\ phase \in {"start", "registered", "overwritten", "removed", "cleared", "scanned", "swept", "done"}
    /\ registered \in BOOLEAN
    /\ scanned \in BOOLEAN
    /\ oldOverwritten \in BOOLEAN
    /\ oldRemoved \in BOOLEAN
    /\ oldCleared \in BOOLEAN
    /\ shadedOverwrite \in BOOLEAN
    /\ shadedRemove \in BOOLEAN
    /\ shadedClear \in BOOLEAN
    /\ freed \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ registered = FALSE
    /\ scanned = FALSE
    /\ oldOverwritten = FALSE
    /\ oldRemoved = FALSE
    /\ oldCleared = FALSE
    /\ shadedOverwrite = FALSE
    /\ shadedRemove = FALSE
    /\ shadedClear = FALSE
    /\ freed = FALSE

RegisterSpace ==
    /\ phase = "start"
    /\ phase' = "registered"
    /\ registered' = TRUE
    /\ UNCHANGED <<scanned, oldOverwritten, oldRemoved, oldCleared,
                  shadedOverwrite, shadedRemove, shadedClear, freed>>

OverwriteSpace ==
    /\ phase = "registered"
    /\ phase' = "overwritten"
    /\ oldOverwritten' = TRUE
    /\ shadedOverwrite' = ShadeOverwrite
    /\ UNCHANGED <<registered, scanned, oldRemoved, oldCleared,
                  shadedRemove, shadedClear, freed>>

RemoveSpace ==
    /\ phase = "overwritten"
    /\ phase' = "removed"
    /\ oldRemoved' = TRUE
    /\ shadedRemove' = ShadeRemove
    /\ UNCHANGED <<registered, scanned, oldOverwritten, oldCleared,
                  shadedOverwrite, shadedClear, freed>>

ClearRegistry ==
    /\ phase = "removed"
    /\ phase' = "cleared"
    /\ oldCleared' = TRUE
    /\ shadedClear' = ShadeClear
    /\ UNCHANGED <<registered, scanned, oldOverwritten, oldRemoved,
                  shadedOverwrite, shadedRemove, freed>>

ScanRegisteredRoots ==
    /\ phase = "cleared"
    /\ phase' = "scanned"
    /\ scanned' = (registered /\ ScanRegistered)
    /\ UNCHANGED <<registered, oldOverwritten, oldRemoved, oldCleared,
                  shadedOverwrite, shadedRemove, shadedClear, freed>>

Sweep ==
    /\ phase = "scanned"
    /\ phase' = "swept"
    /\ freed' =
        ((oldOverwritten /\ ~shadedOverwrite) \/
         (oldRemoved /\ ~shadedRemove) \/
         (oldCleared /\ ~shadedClear) \/
         (registered /\ ~scanned))
    /\ UNCHANGED <<registered, scanned, oldOverwritten, oldRemoved, oldCleared,
                  shadedOverwrite, shadedRemove, shadedClear>>

Finish ==
    /\ phase = "swept"
    /\ phase' = "done"
    /\ UNCHANGED <<registered, scanned, oldOverwritten, oldRemoved, oldCleared,
                  shadedOverwrite, shadedRemove, shadedClear, freed>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ RegisterSpace
    \/ OverwriteSpace
    \/ RemoveSpace
    \/ ClearRegistry
    \/ ScanRegisteredRoots
    \/ Sweep
    \/ Finish
    \/ Done

Spec == Init /\ [][Next]_vars

NoSpaceRegistryValueFreed ==
    ~freed

=============================================================================
