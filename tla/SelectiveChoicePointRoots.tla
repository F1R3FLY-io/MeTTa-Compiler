-------------------------- MODULE SelectiveChoicePointRoots --------------------------
(***************************************************************************)
(* E3 selective-CESK re-enterable continuation roots.                       *)
(***************************************************************************)

EXTENDS FiniteSets

CONSTANTS
    IncludeTrampolineChoice,
    IncludeVmChoice,
    IncludeJitChoice,
    IncludeCapturedSpine

VARIABLES
    phase,
    trampolineChoiceLive,
    vmChoiceLive,
    jitChoiceLive,
    capturedSpineLive,
    trampolineChoiceRooted,
    vmChoiceRooted,
    jitChoiceRooted,
    capturedSpineRooted,
    freed

COMPONENTS == {"trampoline_choice", "vm_choice", "jit_choice", "captured_spine"}

vars ==
    <<phase,
      trampolineChoiceLive, vmChoiceLive, jitChoiceLive, capturedSpineLive,
      trampolineChoiceRooted, vmChoiceRooted, jitChoiceRooted,
      capturedSpineRooted, freed>>

LiveComponents ==
    {c \in COMPONENTS :
        \/ c = "trampoline_choice" /\ trampolineChoiceLive
        \/ c = "vm_choice" /\ vmChoiceLive
        \/ c = "jit_choice" /\ jitChoiceLive
        \/ c = "captured_spine" /\ capturedSpineLive}

RootedComponents ==
    {c \in COMPONENTS :
        \/ c = "trampoline_choice" /\ trampolineChoiceRooted
        \/ c = "vm_choice" /\ vmChoiceRooted
        \/ c = "jit_choice" /\ jitChoiceRooted
        \/ c = "captured_spine" /\ capturedSpineRooted}

Init ==
    /\ phase = "start"
    /\ trampolineChoiceLive = TRUE
    /\ vmChoiceLive = TRUE
    /\ jitChoiceLive = TRUE
    /\ capturedSpineLive = TRUE
    /\ trampolineChoiceRooted = FALSE
    /\ vmChoiceRooted = FALSE
    /\ jitChoiceRooted = FALSE
    /\ capturedSpineRooted = FALSE
    /\ freed = {}

BuildRoots ==
    /\ phase = "start"
    /\ trampolineChoiceRooted' = IncludeTrampolineChoice /\ trampolineChoiceLive
    /\ vmChoiceRooted' = IncludeVmChoice /\ vmChoiceLive
    /\ jitChoiceRooted' = IncludeJitChoice /\ jitChoiceLive
    /\ capturedSpineRooted' = IncludeCapturedSpine /\ capturedSpineLive
    /\ UNCHANGED <<trampolineChoiceLive, vmChoiceLive, jitChoiceLive,
                  capturedSpineLive, freed>>
    /\ phase' = "rooted"

Sweep ==
    /\ phase = "rooted"
    /\ freed' = LiveComponents \ RootedComponents
    /\ UNCHANGED <<trampolineChoiceLive, vmChoiceLive, jitChoiceLive,
                  capturedSpineLive, trampolineChoiceRooted, vmChoiceRooted,
                  jitChoiceRooted, capturedSpineRooted>>
    /\ phase' = "swept"

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next == BuildRoots \/ Sweep \/ Done

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ phase \in {"start", "rooted", "swept"}
    /\ trampolineChoiceLive \in BOOLEAN
    /\ vmChoiceLive \in BOOLEAN
    /\ jitChoiceLive \in BOOLEAN
    /\ capturedSpineLive \in BOOLEAN
    /\ trampolineChoiceRooted \in BOOLEAN
    /\ vmChoiceRooted \in BOOLEAN
    /\ jitChoiceRooted \in BOOLEAN
    /\ capturedSpineRooted \in BOOLEAN
    /\ freed \subseteq COMPONENTS

NoReenterableChoiceFreed ==
    freed \cap LiveComponents = {}

=====================================================================================
