--------------------------- MODULE MC_SlabGC ---------------------------
(*
 * TLC Model Checking Wrapper for SlabGC
 *
 * Instantiates the SlabGC specification with concrete constant values
 * suitable for exhaustive TLC model checking.
 *
 * State space: With MaxSlots=6, MaxRoots=3, MaxExprs=3, the state space
 * is small enough for exhaustive exploration (~millions of states).
 *)

EXTENDS SlabGC

(*
 * TLC uses the .cfg file for constant assignments.
 * This module just extends SlabGC to make it available.
 *
 * The SetToSeq operator is needed by TLC but not built-in.
 * We rely on the TLC module's SetToSeq.
 *)

=======================================================================
