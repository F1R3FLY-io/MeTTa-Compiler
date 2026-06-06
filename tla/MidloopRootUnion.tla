--------------------------- MODULE MidloopRootUnion ---------------------------
(***************************************************************************)
(* Single-threaded mid-loop CESK root-union model.                         *)
(*                                                                         *)
(* At a mid-loop collection point the trampoline is still live. The root   *)
(* vector must therefore include the live S/C/K registers, persistent E0,  *)
(* global anchors, the K-spine, deferred environment drops, and driver-C   *)
(* safepoint roots. Omitting any load-bearing channel lets a live root be   *)
(* swept.                                                                  *)
(***************************************************************************)

CONSTANTS
    IncludeLiveSCK,
    IncludeEnv0,
    IncludeGlobal,
    IncludeKSpine,
    IncludeDeferred,
    IncludeDriverC

VARIABLES
    liveSCK,
    env0Live,
    globalLive,
    kSpineLive,
    deferredLive,
    driverCLive,
    liveSCKRooted,
    env0Rooted,
    globalRooted,
    kSpineRooted,
    deferredRooted,
    driverCRooted,
    built,
    swept

vars ==
    <<liveSCK, env0Live, globalLive, kSpineLive, deferredLive, driverCLive,
      liveSCKRooted, env0Rooted, globalRooted, kSpineRooted,
      deferredRooted, driverCRooted, built, swept>>

TypeOK ==
    /\ liveSCK \in BOOLEAN
    /\ env0Live \in BOOLEAN
    /\ globalLive \in BOOLEAN
    /\ kSpineLive \in BOOLEAN
    /\ deferredLive \in BOOLEAN
    /\ driverCLive \in BOOLEAN
    /\ liveSCKRooted \in BOOLEAN
    /\ env0Rooted \in BOOLEAN
    /\ globalRooted \in BOOLEAN
    /\ kSpineRooted \in BOOLEAN
    /\ deferredRooted \in BOOLEAN
    /\ driverCRooted \in BOOLEAN
    /\ built \in BOOLEAN
    /\ swept \in BOOLEAN

Init ==
    /\ liveSCK = TRUE
    /\ env0Live = TRUE
    /\ globalLive = TRUE
    /\ kSpineLive = TRUE
    /\ deferredLive = TRUE
    /\ driverCLive = TRUE
    /\ liveSCKRooted = FALSE
    /\ env0Rooted = FALSE
    /\ globalRooted = FALSE
    /\ kSpineRooted = FALSE
    /\ deferredRooted = FALSE
    /\ driverCRooted = FALSE
    /\ built = FALSE
    /\ swept = FALSE

BuildRootUnion ==
    /\ ~built
    /\ ~swept
    /\ liveSCKRooted' = IncludeLiveSCK /\ liveSCK
    /\ env0Rooted' = IncludeEnv0 /\ env0Live
    /\ globalRooted' = IncludeGlobal /\ globalLive
    /\ kSpineRooted' = IncludeKSpine /\ kSpineLive
    /\ deferredRooted' = IncludeDeferred /\ deferredLive
    /\ driverCRooted' = IncludeDriverC /\ driverCLive
    /\ built' = TRUE
    /\ UNCHANGED <<liveSCK, env0Live, globalLive, kSpineLive,
                  deferredLive, driverCLive, swept>>

Sweep ==
    /\ built
    /\ ~swept
    /\ swept' = TRUE
    /\ UNCHANGED <<liveSCK, env0Live, globalLive, kSpineLive,
                  deferredLive, driverCLive, liveSCKRooted, env0Rooted,
                  globalRooted, kSpineRooted, deferredRooted, driverCRooted,
                  built>>

Done ==
    /\ swept
    /\ UNCHANGED vars

Next ==
    \/ BuildRootUnion
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

MidloopRootUnionComplete ==
    swept =>
      /\ liveSCK => liveSCKRooted
      /\ env0Live => env0Rooted
      /\ globalLive => globalRooted
      /\ kSpineLive => kSpineRooted
      /\ deferredLive => deferredRooted
      /\ driverCLive => driverCRooted

=============================================================================
