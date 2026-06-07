---------------------------- MODULE FrameEnvRoots ----------------------------
(***************************************************************************)
(* Fork-local environment roots for live CESK frames.                       *)
(*                                                                         *)
(* A forked nondeterministic env can hold values in five local maps that    *)
(* are not necessarily reachable from E0. A live work item or continuation  *)
(* frame must include those map values in its frame root contribution.       *)
(***************************************************************************)

CONSTANTS
    IncludeBindings,
    IncludeTypes,
    IncludeStates,
    IncludeNamedSpaces,
    IncludeInferredTypes

VARIABLES
    built,
    bindingsLive,
    typesLive,
    statesLive,
    namedSpacesLive,
    inferredTypesLive,
    bindingsRooted,
    typesRooted,
    statesRooted,
    namedSpacesRooted,
    inferredTypesRooted,
    swept

vars ==
    <<built, bindingsLive, typesLive, statesLive, namedSpacesLive,
      inferredTypesLive, bindingsRooted, typesRooted, statesRooted,
      namedSpacesRooted, inferredTypesRooted, swept>>

TypeOK ==
    /\ IncludeBindings \in BOOLEAN
    /\ IncludeTypes \in BOOLEAN
    /\ IncludeStates \in BOOLEAN
    /\ IncludeNamedSpaces \in BOOLEAN
    /\ IncludeInferredTypes \in BOOLEAN
    /\ built \in BOOLEAN
    /\ bindingsLive \in BOOLEAN
    /\ typesLive \in BOOLEAN
    /\ statesLive \in BOOLEAN
    /\ namedSpacesLive \in BOOLEAN
    /\ inferredTypesLive \in BOOLEAN
    /\ bindingsRooted \in BOOLEAN
    /\ typesRooted \in BOOLEAN
    /\ statesRooted \in BOOLEAN
    /\ namedSpacesRooted \in BOOLEAN
    /\ inferredTypesRooted \in BOOLEAN
    /\ swept \in BOOLEAN

Init ==
    /\ built = FALSE
    /\ bindingsLive = TRUE
    /\ typesLive = TRUE
    /\ statesLive = TRUE
    /\ namedSpacesLive = TRUE
    /\ inferredTypesLive = TRUE
    /\ bindingsRooted = FALSE
    /\ typesRooted = FALSE
    /\ statesRooted = FALSE
    /\ namedSpacesRooted = FALSE
    /\ inferredTypesRooted = FALSE
    /\ swept = FALSE

BuildFrameRoots ==
    /\ ~built
    /\ ~swept
    /\ bindingsRooted' = IncludeBindings /\ bindingsLive
    /\ typesRooted' = IncludeTypes /\ typesLive
    /\ statesRooted' = IncludeStates /\ statesLive
    /\ namedSpacesRooted' = IncludeNamedSpaces /\ namedSpacesLive
    /\ inferredTypesRooted' = IncludeInferredTypes /\ inferredTypesLive
    /\ built' = TRUE
    /\ UNCHANGED <<bindingsLive, typesLive, statesLive, namedSpacesLive,
                  inferredTypesLive, swept>>

Sweep ==
    /\ built
    /\ ~swept
    /\ swept' = TRUE
    /\ UNCHANGED <<built, bindingsLive, typesLive, statesLive, namedSpacesLive,
                  inferredTypesLive, bindingsRooted, typesRooted, statesRooted,
                  namedSpacesRooted, inferredTypesRooted>>

Done ==
    /\ swept
    /\ UNCHANGED vars

Next ==
    \/ BuildFrameRoots
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

FrameEnvRootsComplete ==
    swept =>
      /\ bindingsLive => bindingsRooted
      /\ typesLive => typesRooted
      /\ statesLive => statesRooted
      /\ namedSpacesLive => namedSpacesRooted
      /\ inferredTypesLive => inferredTypesRooted

=============================================================================
