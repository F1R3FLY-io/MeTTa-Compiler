----------------------- MODULE MidDerivationClear -----------------------
(***************************************************************************)
(* #309/#266 residual root cause #2 (GC-rendezvous clear mid-derivation) — *)
(* formal reproduction + fix verification.                                 *)
(*                                                                         *)
(* While the driver thread is mid-derivation of a subgoal S (a             *)
(* CompleteSubgoal{S} frame pending => S marked active in ACTIVE_EVAL_SET), *)
(* it may park in the dedicated-GC rendezvous and, on RESUME, run           *)
(* clear_all_worker_thread_local_caches (eval_loop.rs:304-322), which       *)
(* clears the SubgoalTable AND ACTIVE_EVAL_SET. Clearing the active set      *)
(* drops the in-flight cycle-cut mark, so a recursive re-entry of S after    *)
(* the clear is no longer detected as a cycle -> it does NOT cut to the      *)
(* fixpoint EMPTY -> a divergent/empty bag is committed by the still-pending *)
(* outer CompleteSubgoal -> a dependent inference layer drops.               *)
(*                                                                         *)
(* The FIX (`ClearGuarded`): skip the fixpoint-memo clear while a frame is   *)
(* pending (ACTIVE_EVAL_SET non-empty). The Addr-keyed value caches still    *)
(* clear; the fixpoint memos are GC-rooted + epoch-revalidated, so skipping  *)
(* introduces no staleness.                                                  *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS ClearGuarded   \* FIX toggle: forbid the clear while a frame is pending

VARIABLES
  active,        \* is subgoal S marked active on this thread (ACTIVE_EVAL_SET)?
  pendingFrame,  \* is a CompleteSubgoal{S} frame still pending (mid-derivation)?
  result         \* "None" | "Correct" | "Wrong"

vars == <<active, pendingFrame, result>>

TypeOK ==
  /\ active \in BOOLEAN
  /\ pendingFrame \in BOOLEAN
  /\ result \in {"None", "Correct", "Wrong"}

(* The thread is mid-deriving S: S is marked active, its CompleteSubgoal is  *)
(* pending, and the recursive re-entry has not happened yet.                 *)
Init ==
  /\ active = TRUE
  /\ pendingFrame = TRUE
  /\ result = "None"

(* GC-rendezvous resume clears the active set — UNLESS guarded while a frame  *)
(* is pending (the fix).                                                      *)
RendezvousClear ==
  /\ active = TRUE
  /\ ~(ClearGuarded /\ pendingFrame)
  /\ active' = FALSE
  /\ UNCHANGED <<pendingFrame, result>>

(* The recursion re-enters S (while the frame is pending). It cuts to the     *)
(* fixpoint (Correct) IFF S is still marked active; if the clear wiped the     *)
(* mark, the cycle is missed and a divergent/empty bag is produced (Wrong).    *)
ReEnter ==
  /\ pendingFrame = TRUE
  /\ result = "None"
  /\ result' = IF active THEN "Correct" ELSE "Wrong"
  /\ UNCHANGED <<active, pendingFrame>>

(* The CompleteSubgoal frame fires: unmark and stop being mid-derivation.     *)
CompleteFrame ==
  /\ pendingFrame = TRUE
  /\ pendingFrame' = FALSE
  /\ active' = FALSE
  /\ UNCHANGED result

Next ==
  \/ RendezvousClear
  \/ ReEnter
  \/ CompleteFrame
  \/ (pendingFrame = FALSE /\ UNCHANGED vars)  \* terminal: derivation done

Spec == Init /\ [][Next]_vars

(* The recursive re-entry must always cut correctly; clearing the active set   *)
(* mid-derivation makes it miss the cut and commit a wrong (dropped-subset)    *)
(* bag. TLC violates this when ClearGuarded=FALSE and proves it when TRUE.     *)
NoDroppedResult == result # "Wrong"
=========================================================================
