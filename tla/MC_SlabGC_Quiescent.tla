------------------------- MODULE MC_SlabGC_Quiescent -------------------------
(*
 * TLC Model Checking Wrapper for SlabGC_Quiescent
 *
 * Instantiates the SlabGC_Quiescent specification with concrete constant values
 * suitable for exhaustive TLC model checking.
 *
 * State space considerations:
 *   - NumEvalThreads=2 captures all 2-thread interleavings (sufficient for
 *     verifying the lock-free coordination protocol)
 *   - MaxSlots=4, MaxRoots=2, MaxExprs=2 keep state space tractable while
 *     exercising allocation, GC, and free-list reuse paths
 *   - The epoch variable and per-slot epoch maps contribute significantly
 *     to state space; keeping MaxSlots small is critical
 *
 * Constants are assigned in the .cfg file.
 *)

EXTENDS SlabGC_Quiescent

(*
 * TLC uses the .cfg file for constant assignments.
 * This module just extends SlabGC_Quiescent to make it available.
 *)

=======================================================================
