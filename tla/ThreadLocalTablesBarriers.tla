------------------------ MODULE ThreadLocalTablesBarriers ------------------------
(***************************************************************************)
(* Thread-local subgoal/thunk table SATB/rooting discriminator.             *)
(*                                                                         *)
(* Subgoal and thunk cached results are thread-local persistent roots read  *)
(* by collect_global_anchors through collect_subgoal_roots and              *)
(* collect_thunk_roots. Removed result values must be shaded during an      *)
(* active SATB mark before sweep can free unmarked values.                  *)
(***************************************************************************)

CONSTANTS
    ScanSubgoal,
    ScanThunk,
    ShadeSubgoalStale,
    ShadeSubgoalOverwrite,
    ShadeSubgoalRemove,
    ShadeSubgoalClear,
    ShadeThunkStale,
    ShadeThunkOverwrite,
    ShadeThunkRemove,
    ShadeThunkClear,
    ShadeThunkReplace

VARIABLES
    phase,
    subgoalRegistered,
    thunkRegistered,
    subgoalScanned,
    thunkScanned,
    oldSubgoalStale,
    oldSubgoalOverwritten,
    oldSubgoalRemoved,
    oldSubgoalCleared,
    oldThunkStale,
    oldThunkOverwritten,
    oldThunkRemoved,
    oldThunkCleared,
    oldThunkReplaced,
    shadedSubgoalStale,
    shadedSubgoalOverwrite,
    shadedSubgoalRemove,
    shadedSubgoalClear,
    shadedThunkStale,
    shadedThunkOverwrite,
    shadedThunkRemove,
    shadedThunkClear,
    shadedThunkReplace,
    freed

vars ==
    <<phase, subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
      oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
      oldSubgoalCleared, oldThunkStale, oldThunkOverwritten,
      oldThunkRemoved, oldThunkCleared, oldThunkReplaced,
      shadedSubgoalStale, shadedSubgoalOverwrite, shadedSubgoalRemove,
      shadedSubgoalClear, shadedThunkStale, shadedThunkOverwrite,
      shadedThunkRemove, shadedThunkClear, shadedThunkReplace, freed>>

TypeOK ==
    /\ phase \in {"start", "registered", "scanned", "subgoal_stale",
                  "subgoal_overwritten", "subgoal_removed", "subgoal_cleared",
                  "thunk_stale", "thunk_overwritten", "thunk_removed",
                  "thunk_replaced", "thunk_cleared", "swept", "done"}
    /\ subgoalRegistered \in BOOLEAN
    /\ thunkRegistered \in BOOLEAN
    /\ subgoalScanned \in BOOLEAN
    /\ thunkScanned \in BOOLEAN
    /\ oldSubgoalStale \in BOOLEAN
    /\ oldSubgoalOverwritten \in BOOLEAN
    /\ oldSubgoalRemoved \in BOOLEAN
    /\ oldSubgoalCleared \in BOOLEAN
    /\ oldThunkStale \in BOOLEAN
    /\ oldThunkOverwritten \in BOOLEAN
    /\ oldThunkRemoved \in BOOLEAN
    /\ oldThunkCleared \in BOOLEAN
    /\ oldThunkReplaced \in BOOLEAN
    /\ shadedSubgoalStale \in BOOLEAN
    /\ shadedSubgoalOverwrite \in BOOLEAN
    /\ shadedSubgoalRemove \in BOOLEAN
    /\ shadedSubgoalClear \in BOOLEAN
    /\ shadedThunkStale \in BOOLEAN
    /\ shadedThunkOverwrite \in BOOLEAN
    /\ shadedThunkRemove \in BOOLEAN
    /\ shadedThunkClear \in BOOLEAN
    /\ shadedThunkReplace \in BOOLEAN
    /\ freed \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ subgoalRegistered = FALSE
    /\ thunkRegistered = FALSE
    /\ subgoalScanned = FALSE
    /\ thunkScanned = FALSE
    /\ oldSubgoalStale = FALSE
    /\ oldSubgoalOverwritten = FALSE
    /\ oldSubgoalRemoved = FALSE
    /\ oldSubgoalCleared = FALSE
    /\ oldThunkStale = FALSE
    /\ oldThunkOverwritten = FALSE
    /\ oldThunkRemoved = FALSE
    /\ oldThunkCleared = FALSE
    /\ oldThunkReplaced = FALSE
    /\ shadedSubgoalStale = FALSE
    /\ shadedSubgoalOverwrite = FALSE
    /\ shadedSubgoalRemove = FALSE
    /\ shadedSubgoalClear = FALSE
    /\ shadedThunkStale = FALSE
    /\ shadedThunkOverwrite = FALSE
    /\ shadedThunkRemove = FALSE
    /\ shadedThunkClear = FALSE
    /\ shadedThunkReplace = FALSE
    /\ freed = FALSE

RegisterTables ==
    /\ phase = "start"
    /\ phase' = "registered"
    /\ subgoalRegistered' = TRUE
    /\ thunkRegistered' = TRUE
    /\ UNCHANGED <<subgoalScanned, thunkScanned, oldSubgoalStale,
                  oldSubgoalOverwritten, oldSubgoalRemoved, oldSubgoalCleared,
                  oldThunkStale, oldThunkOverwritten, oldThunkRemoved,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalRemove, shadedSubgoalClear,
                  shadedThunkStale, shadedThunkOverwrite, shadedThunkRemove,
                  shadedThunkClear, shadedThunkReplace, freed>>

ScanRoots ==
    /\ phase = "registered"
    /\ phase' = "scanned"
    /\ subgoalScanned' = (subgoalRegistered /\ ScanSubgoal)
    /\ thunkScanned' = (thunkRegistered /\ ScanThunk)
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, oldSubgoalStale,
                  oldSubgoalOverwritten, oldSubgoalRemoved, oldSubgoalCleared,
                  oldThunkStale, oldThunkOverwritten, oldThunkRemoved,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalRemove, shadedSubgoalClear,
                  shadedThunkStale, shadedThunkOverwrite, shadedThunkRemove,
                  shadedThunkClear, shadedThunkReplace, freed>>

EvictStaleSubgoal ==
    /\ phase = "scanned"
    /\ phase' = "subgoal_stale"
    /\ oldSubgoalStale' = TRUE
    /\ shadedSubgoalStale' = ShadeSubgoalStale
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalOverwritten, oldSubgoalRemoved, oldSubgoalCleared,
                  oldThunkStale, oldThunkOverwritten, oldThunkRemoved,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalOverwrite,
                  shadedSubgoalRemove, shadedSubgoalClear, shadedThunkStale,
                  shadedThunkOverwrite, shadedThunkRemove, shadedThunkClear,
                  shadedThunkReplace, freed>>

OverwriteSubgoal ==
    /\ phase = "subgoal_stale"
    /\ phase' = "subgoal_overwritten"
    /\ oldSubgoalOverwritten' = TRUE
    /\ shadedSubgoalOverwrite' = ShadeSubgoalOverwrite
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalRemoved, oldSubgoalCleared,
                  oldThunkStale, oldThunkOverwritten, oldThunkRemoved,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalRemove, shadedSubgoalClear, shadedThunkStale,
                  shadedThunkOverwrite, shadedThunkRemove, shadedThunkClear,
                  shadedThunkReplace, freed>>

RemoveSubgoal ==
    /\ phase = "subgoal_overwritten"
    /\ phase' = "subgoal_removed"
    /\ oldSubgoalRemoved' = TRUE
    /\ shadedSubgoalRemove' = ShadeSubgoalRemove
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalCleared,
                  oldThunkStale, oldThunkOverwritten, oldThunkRemoved,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalClear, shadedThunkStale,
                  shadedThunkOverwrite, shadedThunkRemove, shadedThunkClear,
                  shadedThunkReplace, freed>>

ClearSubgoal ==
    /\ phase = "subgoal_removed"
    /\ phase' = "subgoal_cleared"
    /\ oldSubgoalCleared' = TRUE
    /\ shadedSubgoalClear' = ShadeSubgoalClear
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
                  oldThunkStale, oldThunkOverwritten, oldThunkRemoved,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalRemove, shadedThunkStale,
                  shadedThunkOverwrite, shadedThunkRemove, shadedThunkClear,
                  shadedThunkReplace, freed>>

EvictStaleThunk ==
    /\ phase = "subgoal_cleared"
    /\ phase' = "thunk_stale"
    /\ oldThunkStale' = TRUE
    /\ shadedThunkStale' = ShadeThunkStale
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
                  oldSubgoalCleared, oldThunkOverwritten, oldThunkRemoved,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalRemove, shadedSubgoalClear,
                  shadedThunkOverwrite, shadedThunkRemove, shadedThunkClear,
                  shadedThunkReplace, freed>>

OverwriteThunk ==
    /\ phase = "thunk_stale"
    /\ phase' = "thunk_overwritten"
    /\ oldThunkOverwritten' = TRUE
    /\ shadedThunkOverwrite' = ShadeThunkOverwrite
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
                  oldSubgoalCleared, oldThunkStale, oldThunkRemoved,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalRemove, shadedSubgoalClear,
                  shadedThunkStale, shadedThunkRemove, shadedThunkClear,
                  shadedThunkReplace, freed>>

RemoveThunk ==
    /\ phase = "thunk_overwritten"
    /\ phase' = "thunk_removed"
    /\ oldThunkRemoved' = TRUE
    /\ shadedThunkRemove' = ShadeThunkRemove
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
                  oldSubgoalCleared, oldThunkStale, oldThunkOverwritten,
                  oldThunkCleared, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalRemove, shadedSubgoalClear,
                  shadedThunkStale, shadedThunkOverwrite, shadedThunkClear,
                  shadedThunkReplace, freed>>

ReplaceThunkResult ==
    /\ phase = "thunk_removed"
    /\ phase' = "thunk_replaced"
    /\ oldThunkReplaced' = TRUE
    /\ shadedThunkReplace' = ShadeThunkReplace
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
                  oldSubgoalCleared, oldThunkStale, oldThunkOverwritten,
                  oldThunkRemoved, oldThunkCleared, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalRemove, shadedSubgoalClear,
                  shadedThunkStale, shadedThunkOverwrite, shadedThunkRemove,
                  shadedThunkClear, freed>>

ClearThunk ==
    /\ phase = "thunk_replaced"
    /\ phase' = "thunk_cleared"
    /\ oldThunkCleared' = TRUE
    /\ shadedThunkClear' = ShadeThunkClear
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
                  oldSubgoalCleared, oldThunkStale, oldThunkOverwritten,
                  oldThunkRemoved, oldThunkReplaced, shadedSubgoalStale,
                  shadedSubgoalOverwrite, shadedSubgoalRemove, shadedSubgoalClear,
                  shadedThunkStale, shadedThunkOverwrite, shadedThunkRemove,
                  shadedThunkReplace, freed>>

Sweep ==
    /\ phase = "thunk_cleared"
    /\ phase' = "swept"
    /\ freed' =
        ((subgoalRegistered /\ ~subgoalScanned) \/
         (thunkRegistered /\ ~thunkScanned) \/
         (oldSubgoalStale /\ ~shadedSubgoalStale) \/
         (oldSubgoalOverwritten /\ ~shadedSubgoalOverwrite) \/
         (oldSubgoalRemoved /\ ~shadedSubgoalRemove) \/
         (oldSubgoalCleared /\ ~shadedSubgoalClear) \/
         (oldThunkStale /\ ~shadedThunkStale) \/
         (oldThunkOverwritten /\ ~shadedThunkOverwrite) \/
         (oldThunkRemoved /\ ~shadedThunkRemove) \/
         (oldThunkCleared /\ ~shadedThunkClear) \/
         (oldThunkReplaced /\ ~shadedThunkReplace))
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
                  oldSubgoalCleared, oldThunkStale, oldThunkOverwritten,
                  oldThunkRemoved, oldThunkCleared, oldThunkReplaced,
                  shadedSubgoalStale, shadedSubgoalOverwrite, shadedSubgoalRemove,
                  shadedSubgoalClear, shadedThunkStale, shadedThunkOverwrite,
                  shadedThunkRemove, shadedThunkClear, shadedThunkReplace>>

Finish ==
    /\ phase = "swept"
    /\ phase' = "done"
    /\ UNCHANGED <<subgoalRegistered, thunkRegistered, subgoalScanned, thunkScanned,
                  oldSubgoalStale, oldSubgoalOverwritten, oldSubgoalRemoved,
                  oldSubgoalCleared, oldThunkStale, oldThunkOverwritten,
                  oldThunkRemoved, oldThunkCleared, oldThunkReplaced,
                  shadedSubgoalStale, shadedSubgoalOverwrite, shadedSubgoalRemove,
                  shadedSubgoalClear, shadedThunkStale, shadedThunkOverwrite,
                  shadedThunkRemove, shadedThunkClear, shadedThunkReplace, freed>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ RegisterTables
    \/ ScanRoots
    \/ EvictStaleSubgoal
    \/ OverwriteSubgoal
    \/ RemoveSubgoal
    \/ ClearSubgoal
    \/ EvictStaleThunk
    \/ OverwriteThunk
    \/ RemoveThunk
    \/ ReplaceThunkResult
    \/ ClearThunk
    \/ Sweep
    \/ Finish
    \/ Done

Spec == Init /\ [][Next]_vars

NoThreadLocalTableValueFreed ==
    ~freed

=============================================================================
