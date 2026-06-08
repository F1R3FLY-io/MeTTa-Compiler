------------------------ MODULE SerializableContinuationSlice ------------------------
(***************************************************************************)
(* E4 serialized continuation store-slice completeness.                    *)
(***************************************************************************)

EXTENDS FiniteSets

CONSTANTS
    IncludeControl,
    IncludeEnv,
    IncludeKont,
    IncludeReachableChild

VARIABLES
    phase,
    controlLive,
    envLive,
    kontLive,
    childLive,
    controlInSlice,
    envInSlice,
    kontInSlice,
    childInSlice,
    freed

COMPONENTS == {"control", "env", "kont", "child"}

SeedLive ==
    {c \in COMPONENTS :
        \/ c = "control" /\ controlLive
        \/ c = "env" /\ envLive
        \/ c = "kont" /\ kontLive}

FutureTouches ==
    SeedLive \cup {c \in COMPONENTS : c = "child" /\ childLive}

SliceComponents ==
    {c \in COMPONENTS :
        \/ c = "control" /\ controlInSlice
        \/ c = "env" /\ envInSlice
        \/ c = "kont" /\ kontInSlice
        \/ c = "child" /\ childInSlice}

vars ==
    <<phase, controlLive, envLive, kontLive, childLive,
      controlInSlice, envInSlice, kontInSlice, childInSlice, freed>>

Init ==
    /\ phase = "start"
    /\ controlLive = TRUE
    /\ envLive = TRUE
    /\ kontLive = TRUE
    /\ childLive = TRUE
    /\ controlInSlice = FALSE
    /\ envInSlice = FALSE
    /\ kontInSlice = FALSE
    /\ childInSlice = FALSE
    /\ freed = {}

BuildSlice ==
    /\ phase = "start"
    /\ controlInSlice' = IncludeControl /\ controlLive
    /\ envInSlice' = IncludeEnv /\ envLive
    /\ kontInSlice' = IncludeKont /\ kontLive
    /\ childInSlice' = IncludeReachableChild /\ childLive
    /\ UNCHANGED <<controlLive, envLive, kontLive, childLive, freed>>
    /\ phase' = "serialized"

RestoreAndSweep ==
    /\ phase = "serialized"
    /\ freed' = FutureTouches \ SliceComponents
    /\ UNCHANGED <<controlLive, envLive, kontLive, childLive,
                  controlInSlice, envInSlice, kontInSlice, childInSlice>>
    /\ phase' = "swept"

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next == BuildSlice \/ RestoreAndSweep \/ Done

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ phase \in {"start", "serialized", "swept"}
    /\ controlLive \in BOOLEAN
    /\ envLive \in BOOLEAN
    /\ kontLive \in BOOLEAN
    /\ childLive \in BOOLEAN
    /\ controlInSlice \in BOOLEAN
    /\ envInSlice \in BOOLEAN
    /\ kontInSlice \in BOOLEAN
    /\ childInSlice \in BOOLEAN
    /\ freed \subseteq COMPONENTS

NoRestoredFutureTouchFreed ==
    freed \cap FutureTouches = {}

=====================================================================================
