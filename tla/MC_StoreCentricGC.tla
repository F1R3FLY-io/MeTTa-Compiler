------------------------- MODULE MC_StoreCentricGC -------------------------
(*****************************************************************************)
(* TLC Model-Checking Wrapper for StoreCentricGC.                            *)
(*                                                                           *)
(* Instantiates the StoreCentricGC specification with concrete, SMALL        *)
(* constant values suitable for EXHAUSTIVE TLC model checking.               *)
(*                                                                           *)
(* State-space considerations (mirrors the slab models' tractability notes): *)
(*   - SEGMENTS = {0, 1}, OFFSETS = {0, 1}  => 4 Addrs in 2 segments of 2     *)
(*     offsets each. Exercises: free-list reuse, partial-segment occupancy,   *)
(*     and WHOLESALE SEGMENT RELEASE (one 2-Addr segment can go fully dead    *)
(*     and be released, while the other stays partly live).                  *)
(*   - NumWorkers = 2 captures the multi-mutator quiescence rendezvous       *)
(*     (the collector must wait for BOTH workers to park before marking).    *)
(*   - MaxRoots = 3 bounds |psi| while leaving headroom for a NON-TRIVIAL    *)
(*     graph: two roots A,B plus the ability to RemoveRoot(B) so B remains    *)
(*     a purely-transitively-reachable child of A — this is what makes the    *)
(*     multi-level transitive MARK reachable (confirmed by coverage probes).  *)
(*                                                                           *)
(* The dominant state-space drivers are `store` (3^4 = 81 base occupancy      *)
(* maps) and `edges` (each of 4 Addrs may hold any subset of up to 4 Addrs).  *)
(* `edges` is the heavy contributor. Raising OFFSETS to {0,1,2} (6 Addrs)     *)
(* makes `edges` (6 x 2^6) explode — the BFS frontier grows past 200M and a   *)
(* COMPLETE run becomes intractable; that larger config was run depth-bounded *)
(* as a partial-confidence check only (see RESULTS.md). The 4-Addr config     *)
(* completes the FULL reachable graph (queue -> 0) in ~80s.                   *)
(*                                                                           *)
(* Constants are assigned in MC_StoreCentricGC.cfg.                          *)
(*****************************************************************************)

EXTENDS StoreCentricGC

(*
 * This module just extends StoreCentricGC so the concrete constants from the
 * .cfg file are in scope. No overrides are needed — the spec is self-
 * contained and the .cfg supplies SEGMENTS, OFFSETS, NumWorkers, MaxRoots.
 *)

=============================================================================
