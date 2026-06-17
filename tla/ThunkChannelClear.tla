----------------------- MODULE ThunkChannelClear -----------------------
(***************************************************************************)
(* #309/#266 residual (thunk channel of root cause #2) — formal            *)
(* reproduction + fix verification.                                        *)
(*                                                                         *)
(* FIX #2 skips the GC-rendezvous resume's fixpoint-memo clear while the    *)
(* thread is mid-derivation. But its predicate, `active_eval_set_is_empty`, *)
(* checks ONLY `ACTIVE_EVAL_SET`, which tracks SUBGOALS. A thunk in         *)
(* Blackhole (a `CompleteThunk` derivation in flight) is NOT in             *)
(* `ACTIVE_EVAL_SET` (thunks never call `mark_eval_active`). So when a       *)
(* worker is mid-THUNK-derivation and no subgoal frame happens to be        *)
(* pending, the subgoal guard falls through and `clear_thunk_table()` wipes  *)
(* the live blackhole -> a sibling re-use of the same (template,bindings)    *)
(* misses the memo/cut -> a clean SUBSET of results drops (no runaway, no    *)
(* error: the subgoal channel is already guarded, so this is the quiet      *)
(* residual).                                                                *)
(*                                                                         *)
(* The FIX (`ThunkGuardChecksThunk`): gate the THUNK clear on the THUNK's    *)
(* own blackhole state (`thunk_table_has_blackhole`), independent of the     *)
(* subgoal active set.                                                       *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS ThunkGuardChecksThunk  \* FIX toggle: gate the thunk clear on the THUNK
                                 \* state (TRUE) vs. share the SUBGOAL guard (FALSE)

VARIABLES
  thkActive,    \* is the thunk marked Blackhole (being evaluated)?
  thkPending,   \* is a CompleteThunk frame still pending (mid-thunk-derivation)?
  subPending,   \* is a SUBGOAL frame pending (the buggy guard's predicate)?
  result        \* "None" | "Correct" | "Wrong"

vars == <<thkActive, thkPending, subPending, result>>

TypeOK ==
  /\ thkActive \in BOOLEAN
  /\ thkPending \in BOOLEAN
  /\ subPending \in BOOLEAN
  /\ result \in {"None", "Correct", "Wrong"}

(* A worker mid-THUNK-derivation (thunk blackholed, CompleteThunk pending)    *)
(* with NO subgoal frame pending — exactly the standalone-thunk case the      *)
(* subgoal guard misses.                                                      *)
Init ==
  /\ thkActive = TRUE
  /\ thkPending = TRUE
  /\ subPending = FALSE
  /\ result = "None"

(* GC-rendezvous resume clears the thunk's blackhole. The guard skips the     *)
(* clear while mid-derivation — but on the WRONG channel when buggy:          *)
(*   fix : skip while the THUNK frame is pending  (~thkPending)               *)
(*   bug : skip while the SUBGOAL frame is pending (~subPending) — never true  *)
(*         here, so the clear fires and wipes the live thunk mark.            *)
ClearThunk ==
  /\ thkActive = TRUE
  /\ LET allowed == IF ThunkGuardChecksThunk THEN ~thkPending ELSE ~subPending
     IN allowed
  /\ thkActive' = FALSE
  /\ UNCHANGED <<thkPending, subPending, result>>

(* The thunk recursion re-enters; it cuts to the fixpoint (Correct) iff the   *)
(* thunk is still Blackhole; if the clear wiped it, the cut is missed and a    *)
(* divergent/smaller bag is produced (Wrong).                                  *)
ReEnterThunk ==
  /\ thkPending = TRUE
  /\ result = "None"
  /\ result' = IF thkActive THEN "Correct" ELSE "Wrong"
  /\ UNCHANGED <<thkActive, thkPending, subPending>>

CompleteThunkFrame ==
  /\ thkPending = TRUE
  /\ thkPending' = FALSE
  /\ thkActive' = FALSE
  /\ UNCHANGED <<subPending, result>>

Next ==
  \/ ClearThunk
  \/ ReEnterThunk
  \/ CompleteThunkFrame
  \/ (thkPending = FALSE /\ UNCHANGED vars)   \* terminal: thunk derivation done

Spec == Init /\ [][Next]_vars

(* The thunk re-entry must always cut correctly; sharing the subgoal guard     *)
(* lets the clear wipe a live thunk mark and drop a layer. TLC violates this   *)
(* when ThunkGuardChecksThunk=FALSE and proves it when TRUE.                    *)
NoDroppedResult == result # "Wrong"
=======================================================================
