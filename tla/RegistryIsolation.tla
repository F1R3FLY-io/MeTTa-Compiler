--------------------------- MODULE RegistryIsolation ---------------------------
(***************************************************************************)
(* Index-mode root-source isolation.                                        *)
(*                                                                         *)
(* The CESK index collector may use structural machine roots and explicit   *)
(* driver transport roots. The old RootProvider registry is a slab bridge   *)
(* only; enabling it as an index root source violates the architecture.      *)
(***************************************************************************)

CONSTANTS
    IndexMode,
    RegistryEnabled,
    IncludeStructural,
    IncludeDriver

VARIABLES
    built,
    structuralRooted,
    driverRooted,
    registryRooted,
    swept

vars ==
    <<built, structuralRooted, driverRooted, registryRooted, swept>>

TypeOK ==
    /\ IndexMode \in BOOLEAN
    /\ RegistryEnabled \in BOOLEAN
    /\ IncludeStructural \in BOOLEAN
    /\ IncludeDriver \in BOOLEAN
    /\ built \in BOOLEAN
    /\ structuralRooted \in BOOLEAN
    /\ driverRooted \in BOOLEAN
    /\ registryRooted \in BOOLEAN
    /\ swept \in BOOLEAN

Init ==
    /\ built = FALSE
    /\ structuralRooted = FALSE
    /\ driverRooted = FALSE
    /\ registryRooted = FALSE
    /\ swept = FALSE

BuildRootSet ==
    /\ ~built
    /\ ~swept
    /\ structuralRooted' = IncludeStructural
    /\ driverRooted' = IncludeDriver
    /\ registryRooted' = RegistryEnabled
    /\ built' = TRUE
    /\ UNCHANGED swept

Sweep ==
    /\ built
    /\ ~swept
    /\ swept' = TRUE
    /\ UNCHANGED <<built, structuralRooted, driverRooted, registryRooted>>

Done ==
    /\ swept
    /\ UNCHANGED vars

Next ==
    \/ BuildRootSet
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoRegistryInIndex ==
    IndexMode => ~registryRooted

IndexRootSetCompleteWithoutRegistry ==
    swept =>
      /\ structuralRooted
      /\ driverRooted
      /\ ~registryRooted

=============================================================================
