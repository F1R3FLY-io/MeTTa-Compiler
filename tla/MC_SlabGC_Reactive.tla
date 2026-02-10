--------------------------- MODULE MC_SlabGC_Reactive ---------------------------
(*
 * TLC Model Checking Wrapper for SlabGC_Reactive
 *
 * Instantiates the SlabGC_Reactive specification with concrete constant values
 * suitable for exhaustive TLC model checking.
 *
 * State space: With MaxSlots=6, MaxRoots=3, MaxExprs=3, the state space
 * is larger than the original spec due to epoch and slotEpoch variables,
 * but still tractable for exhaustive exploration.
 *)

EXTENDS SlabGC_Reactive

(*
 * TLC uses the .cfg file for constant assignments.
 * This module just extends SlabGC_Reactive to make it available.
 *)

=======================================================================
