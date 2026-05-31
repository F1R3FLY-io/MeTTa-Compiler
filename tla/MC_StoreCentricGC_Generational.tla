----------------- MODULE MC_StoreCentricGC_Generational -----------------
(*****************************************************************************)
(* TLC model-checking wrapper for StoreCentricGC_Generational (C1.b).        *)
(*                                                                           *)
(* SMALL exhaustive model: SEGMENTS = {0,1}, OFFSETS = {0,1} => 4 Addrs in   *)
(* 2 segments of 2 offsets. youngFloor in 0..2 exercises: all-young (0),      *)
(* seg-0-old/seg-1-young (1), all-old (2) — so the model reaches states with  *)
(* a genuine OLD segment, an OLD->YOUNG edge (old parent reachable from a     *)
(* root, pointing at a freshly-young child via RewireEdge), and a MINOR sweep *)
(* over the young segment while old garbage is retained.                     *)
(*                                                                           *)
(* MaxRoots = 3 leaves headroom for a NON-TRIVIAL transitive graph (root A    *)
(* with a transitively-reachable child B after RemoveRoot(B)) so the FULL     *)
(* mark's multi-level closure is exercised, AND for an old parent + young     *)
(* child both reachable. Single mutator at quiescence (no NumWorkers — C1.b   *)
(* runs at the base spec's already-proven single-threaded quiescence).       *)
(*                                                                           *)
(* Constants are assigned in MC_StoreCentricGC_Generational.cfg.             *)
(*****************************************************************************)

EXTENDS StoreCentricGC_Generational

=============================================================================
