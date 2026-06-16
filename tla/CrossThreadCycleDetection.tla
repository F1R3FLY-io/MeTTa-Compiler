-------------------- MODULE CrossThreadCycleDetection --------------------
(***************************************************************************)
(* #309/#266 residual root cause #1 (nested-fanout cross-thread cycle-     *)
(* detection gap) — formal reproduction + fix verification.                *)
(*                                                                         *)
(* MeTTaTron tables recursive subgoals and cuts a recursion when the same  *)
(* subgoal S is re-entered while still being evaluated, detected via a      *)
(* THREAD-LOCAL active set (`ACTIVE_EVAL_SET`, tabling.rs:49). Single-      *)
(* threaded, the recursive re-entry of S is on the SAME thread that holds   *)
(* S active, so it is cut immediately at the first re-entry — that cut's    *)
(* empty contribution is exactly what makes the PLN fixpoint converge to    *)
(* its correct result bag.                                                  *)
(*                                                                         *)
(* Under FANOUT, S's recursive sub-derivation can be dispatched to ANOTHER  *)
(* worker (eval_loop.rs:2707/3517), which starts a fresh trampoline with an *)
(* EMPTY active set (it is not seeded from the parent). That worker does    *)
(* not see S as active, so it MISSES the immediate cut and instead descends *)
(* (re-evaluating S afresh, possibly bouncing across more workers). The cut *)
(* is delayed or replaced by a depth-limit StackOverflow; either way the    *)
(* tabled bag for S differs from (is smaller than) the single-threaded      *)
(* fixpoint, and a later use of S reads the wrong bag -> a clean SUBSET of  *)
(* results is dropped.                                                      *)
(*                                                                         *)
(* This model abstracts the recursion to the property that matters: the     *)
(* recursion is correct IFF it is cut at the FIRST re-entry (as single-     *)
(* threaded does). `SeedActiveOnFanout` is the FIX: a fanned-out worker     *)
(* inherits the parent's active set, so the cross-thread re-entry is seen   *)
(* as a cycle and cut immediately.                                          *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
  Threads,             \* set of worker threads, e.g. {t1, t2}
  MaxReentries,        \* recursion bound (StackOverflow boundary); e.g. 3
  SeedActiveOnFanout   \* FIX toggle: propagate the active set across fanout

VARIABLES
  depth,        \* number of recursive re-entries taken so far
  curThread,    \* thread evaluating the current deepest S frame
  active,       \* [Threads -> BOOLEAN] : is S active on this thread's local stack?
  result        \* "None" | "Correct" | "Wrong"

vars == <<depth, curThread, active, result>>

Results == {"None", "Correct", "Wrong"}

TypeOK ==
  /\ depth \in 0..MaxReentries
  /\ curThread \in Threads
  /\ active \in [Threads -> BOOLEAN]
  /\ result \in Results

(* Initially some worker starts evaluating S; only that worker has S active. *)
Init ==
  /\ depth = 0
  /\ curThread \in Threads
  /\ active = [t \in Threads |-> (t = curThread)]
  /\ result = "None"

(* The recursion re-enters S, dispatched to thread `next`.                   *)
(*   next = curThread : sequential (same-thread) recursive descent.          *)
(*   next # curThread : a FANOUT of the recursive sub-derivation.            *)
ReEnter(next) ==
  /\ result = "None"
  /\ LET seen ==
           \/ active[next]                         \* cycle visible on next's own stack
           \/ ( next # curThread                   \* ...or the FIX: a fanned-out worker
                /\ SeedActiveOnFanout               \*    inherits the parent's active set,
                /\ active[curThread] )              \*    so the global recursion is visible
     IN IF seen
          THEN \* cycle detected -> CUT. Correct iff cut at the first re-entry
               \* (depth 0), matching single-threaded; a delayed cut tabled a
               \* divergent (smaller) bag.
               /\ result' = IF depth = 0 THEN "Correct" ELSE "Wrong"
               /\ UNCHANGED <<depth, curThread, active>>
          ELSE IF depth + 1 >= MaxReentries
                 THEN \* recursion bound reached without a cut -> StackOverflow/divergent
                      /\ result' = "Wrong"
                      /\ UNCHANGED <<depth, curThread, active>>
                 ELSE \* descend one level on `next` (which now marks S active)
                      /\ depth' = depth + 1
                      /\ curThread' = next
                      /\ active' = [active EXCEPT ![next] = TRUE]
                      /\ UNCHANGED result

Next ==
  \/ \E next \in Threads : ReEnter(next)
  \/ (result # "None" /\ UNCHANGED vars)   \* terminal stutter

Spec == Init /\ [][Next]_vars

(* The safety property: the recursion is always cut correctly (single-       *)
(* threaded behavior); it is NEVER tabled as a divergent/smaller bag.        *)
(* TLC violates this on the buggy model (a fanned-out first re-entry misses  *)
(* the cut) and proves it on the fixed model (SeedActiveOnFanout = TRUE).    *)
NoDroppedResult == result # "Wrong"
=============================================================================
