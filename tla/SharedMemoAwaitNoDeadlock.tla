--------------------- MODULE SharedMemoAwaitNoDeadlock ---------------------
(***************************************************************************)
(* #309/#266 architectural fix (shared memo store) — the load-bearing      *)
(* safety property of the await protocol: it CANNOT deadlock against the    *)
(* parent-blocks-on-workers merge, and it does NOT over-cut a shared        *)
(* in-flight dependency. See docs/design/SHARED_MEMO_STORE_309.md §4-5.      *)
(*                                                                         *)
(* Setup: a requester derivation `R` needs an in-flight thunk owned by      *)
(* `O`. The wait-for graph already records the parent-blocks-on-workers     *)
(* edges: if `O` is an ANCESTOR of `R` (the canonical hazard — the blocked  *)
(* parent owns a thunk its own fanned-out worker needs), then `O` is parked *)
(* awaiting `R` (edge `O -> R`), because the parent's collapse merge blocks  *)
(* on `remaining` until `R` finishes.                                       *)
(*                                                                         *)
(* R's decision:                                                            *)
(*   bug  (CheckCycle = FALSE): blindly PARK awaiting O — adds `R -> O`.     *)
(*        With `O -> R` already present that closes the cycle O<->R: the     *)
(*        worker parks on the blocked parent and BOTH wedge -> DEADLOCK.     *)
(*   fix  (CheckCycle = TRUE): test `would_cycle(R -> O)` first. If awaiting  *)
(*        would close a wait-for cycle, do NOT park: CUT (when O is a        *)
(*        genuine ancestor = a real fixpoint cycle) or LOCAL-EVAL (when O is  *)
(*        a sibling = a shared dependency). Either way no `R -> O` edge ->    *)
(*        no cycle, and a shared dependency is never cut.                    *)
(*                                                                         *)
(* TLC: the bug cfg reaches `O` ancestor and VIOLATES `NoDeadlock`; the fix  *)
(* cfg holds `NoDeadlock` AND `NoOverCut` for BOTH `O` ancestor and sibling. *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS CheckCycle  \* FIX toggle: test would_cycle before parking (TRUE) vs park blindly (FALSE)

VARIABLES
  ownerIsAncestor, \* is O an ancestor of R (a real cross-thread cycle) or a sibling (shared dep)?
  waitEdges,       \* set of <<from, to>> wait-for edges currently parked
  decision         \* R's outcome: "pending" | "await" | "cut" | "localeval"

vars == <<ownerIsAncestor, waitEdges, decision>>

TypeOK ==
  /\ ownerIsAncestor \in BOOLEAN
  /\ decision \in {"pending", "await", "cut", "localeval"}

(* Initially: O owns an INFLIGHT thunk R needs. If O is R's ancestor, the   *)
(* parent-merge already parks O awaiting R (edge O->R). Explore both         *)
(* relationships (ancestor / sibling) so the fix's no-over-cut is checked.   *)
Init ==
  /\ ownerIsAncestor \in {TRUE, FALSE}
  /\ waitEdges = (IF ownerIsAncestor THEN {<<"O", "R">>} ELSE {})
  /\ decision = "pending"

(* R decides. `would_cycle(R->O)` is true iff R is already reachable from O   *)
(* — here, directly, iff edge O->R is present (O parked awaiting R).          *)
RDecide ==
  /\ decision = "pending"
  /\ LET wouldCycle == (<<"O", "R">> \in waitEdges)
     IN IF CheckCycle /\ wouldCycle
          THEN \* awaiting would deadlock: break it without parking
               /\ decision' = (IF ownerIsAncestor THEN "cut" ELSE "localeval")
               /\ UNCHANGED waitEdges
          ELSE \* park awaiting O
               /\ decision' = "await"
               /\ waitEdges' = (waitEdges \cup {<<"R", "O">>})
  /\ UNCHANGED ownerIsAncestor

Next == RDecide \/ (decision # "pending" /\ UNCHANGED vars)

Spec == Init /\ [][Next]_vars

(* A parked deadlock is a 2-cycle O<->R in the wait-for graph. *)
HasDeadlockCycle == (<<"O", "R">> \in waitEdges) /\ (<<"R", "O">> \in waitEdges)
NoDeadlock == ~HasDeadlockCycle

(* A cut is emitted only for a genuine cross-thread cycle (O an ancestor),    *)
(* never for a shared in-flight dependency (O a sibling).                     *)
NoOverCut == (decision = "cut") => ownerIsAncestor
=========================================================================
