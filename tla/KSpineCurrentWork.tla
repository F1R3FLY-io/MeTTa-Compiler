---- MODULE KSpineCurrentWork ----

EXTENDS FiniteSets

CONSTANTS IncludeCurrentWork, IncludeWorkStack, IncludeKont

VARIABLES phase,
          currentWorkLive, workStackLive, kontLive,
          currentWorkRooted, workStackRooted, kontRooted,
          freed

CONTROLS == {"current", "work_stack", "kont"}

LiveControls ==
    {c \in CONTROLS :
        \/ c = "current" /\ currentWorkLive
        \/ c = "work_stack" /\ workStackLive
        \/ c = "kont" /\ kontLive}

RootedControls ==
    {c \in CONTROLS :
        \/ c = "current" /\ currentWorkRooted
        \/ c = "work_stack" /\ workStackRooted
        \/ c = "kont" /\ kontRooted}

Init ==
  /\ phase = "start"
  /\ currentWorkLive = TRUE
  /\ workStackLive = TRUE
  /\ kontLive = TRUE
  /\ currentWorkRooted = FALSE
  /\ workStackRooted = FALSE
  /\ kontRooted = FALSE
  /\ freed = {}

BuildRoots ==
  /\ phase = "start"
  /\ currentWorkRooted' = IncludeCurrentWork /\ currentWorkLive
  /\ workStackRooted' = IncludeWorkStack /\ workStackLive
  /\ kontRooted' = IncludeKont /\ kontLive
  /\ UNCHANGED <<currentWorkLive, workStackLive, kontLive, freed>>
  /\ phase' = "rooted"

Sweep ==
  /\ phase = "rooted"
  /\ freed' = LiveControls \ RootedControls
  /\ UNCHANGED <<currentWorkLive, workStackLive, kontLive,
                  currentWorkRooted, workStackRooted, kontRooted>>
  /\ phase' = "swept"

Done ==
  /\ phase = "swept"
  /\ UNCHANGED <<phase, currentWorkLive, workStackLive, kontLive,
                  currentWorkRooted, workStackRooted, kontRooted, freed>>

Next == BuildRoots \/ Sweep \/ Done

Spec == Init /\ [][Next]_<<phase,
                         currentWorkLive, workStackLive, kontLive,
                         currentWorkRooted, workStackRooted, kontRooted,
                         freed>>

TypeOK ==
  /\ phase \in {"start", "rooted", "swept"}
  /\ currentWorkLive \in BOOLEAN
  /\ workStackLive \in BOOLEAN
  /\ kontLive \in BOOLEAN
  /\ currentWorkRooted \in BOOLEAN
  /\ workStackRooted \in BOOLEAN
  /\ kontRooted \in BOOLEAN
  /\ freed \subseteq CONTROLS

NoLiveControlFreed ==
  freed \cap LiveControls = {}

====
