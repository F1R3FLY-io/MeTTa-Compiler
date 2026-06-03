---------------------------- MODULE CollapseCompletion ----------------------------
(***************************************************************************)
(* Liveness model of MeTTaTron's parallel-(collapse) completion handshake. *)
(*                                                                         *)
(* Models the DOMINANT ~2% robot hang under FANOUT>0 + the dedicated index *)
(* collector, root-caused 2026-06-03 via a SIGUSR1 thread-dump (all        *)
(* threads futex-sleeping, no GC in flight) + source analysis:             *)
(*                                                                         *)
(*   parallel_collapse_dispatch (eval_loop.rs:3356) spawns `N = num_items` *)
(*   workers and creates `remaining = AtomicU32(N)`. A worker decrements   *)
(*   `remaining` at the SINGLE site eval_loop.rs:3612 (the closure tail);  *)
(*   the worker that drives remaining 1->0 sets `done`. The parent pumps   *)
(*   `cvar.wait_timeout(done, 100us)` and the WaitForParallelCollapse arm  *)
(*   (eval_loop.rs:15756) exits ONLY when `remaining = 0` (cancel is       *)
(*   Demand::All for plain collapse, never satisfied).                     *)
(*                                                                         *)
(* THE BUG: the collapse worker runs eval_trampoline_with_carrying         *)
(*   (eval_loop.rs:3555) with NO catch_unwind. A panic there (e.g. a stale *)
(*   swept Addr -> .expect/debug_assert) unwinds PAST the decrement; the   *)
(*   pool's catch (priority_scheduler.rs:485) swallows it and the worker   *)
(*   goes idle. `remaining` is then stuck >= 1 forever; `done` is never    *)
(*   set; the parent pumps forever; all workers idle. Permanent hang.      *)
(*                                                                         *)
(* THE FIX (FixApplied = TRUE): a CompletionGuard RAII at the top of the   *)
(*   worker closure decrements `remaining`/sets `done` in its Drop, so the *)
(*   decrement fires on the unwind (panic) edge too -> exactly-once on     *)
(*   EVERY exit path. Modeled by making Panic also decrement.              *)
(*                                                                         *)
(* TLC result (see CollapseCompletion.cfg): the temporal property          *)
(*   EventuallyDone == <>(parentDone) is VIOLATED when FixApplied = FALSE   *)
(*   (the panic-then-stuck lasso), and HOLDS when FixApplied = TRUE.        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    N,           \* number of collapse workers (= num_items)
    FixApplied   \* TRUE  = CompletionGuard fix (Panic also decrements)
                 \* FALSE = the bug (Panic skips the decrement)

VARIABLES
    remaining,   \* AtomicU32: count of workers that have NOT yet decremented (0..N)
    done,        \* the done flag (set by the worker that drives remaining 1->0)
    wstate,      \* [1..N -> {"running","idle"}] : per-worker liveness state
    parentDone   \* the parent's WaitForParallelCollapse arm has observed remaining = 0

vars == <<remaining, done, wstate, parentDone>>

TypeOK ==
    /\ remaining \in 0..N
    /\ done \in BOOLEAN
    /\ wstate \in [1..N -> {"running", "idle"}]
    /\ parentDone \in BOOLEAN

Init ==
    /\ remaining = N
    /\ done = FALSE
    /\ wstate = [w \in 1..N |-> "running"]
    /\ parentDone = FALSE

(* A worker that exits via the NORMAL closure tail: decrement, and the     *)
(* worker that drives remaining 1->0 sets `done`. (eval_loop.rs:3612-3617) *)
Finish(w) ==
    /\ wstate[w] = "running"
    /\ wstate'   = [wstate EXCEPT ![w] = "idle"]
    /\ remaining' = remaining - 1
    /\ done'      = (remaining = 1) \/ done
    /\ UNCHANGED parentDone

(* A worker whose eval PANICS. The unwind is swallowed by the pool and the *)
(* worker goes idle. BUG (FixApplied=FALSE): the decrement at :3612 is     *)
(* bypassed. FIX (FixApplied=TRUE): the CompletionGuard's Drop decrements  *)
(* on the unwind edge too.                                                 *)
Panic(w) ==
    /\ wstate[w] = "running"
    /\ wstate'   = [wstate EXCEPT ![w] = "idle"]
    /\ IF FixApplied
         THEN /\ remaining' = remaining - 1
              /\ done'      = (remaining = 1) \/ done
         ELSE /\ remaining' = remaining   \* THE BUG: decrement skipped
              /\ done'      = done
    /\ UNCHANGED parentDone

(* The parent's WaitForParallelCollapse arm: it pumps wait_timeout(100us)  *)
(* and exits only when remaining = 0 (eval_loop.rs:15756).                 *)
ParentObservesDone ==
    /\ remaining = 0
    /\ ~parentDone
    /\ parentDone' = TRUE
    /\ UNCHANGED <<remaining, done, wstate>>

(* Stutter once everything is idle, so a terminal/stuck state is not       *)
(* flagged as a (safety) deadlock -- the LIVENESS property EventuallyDone   *)
(* is then the discriminator: it HOLDS for the fix (parentDone is forced    *)
(* true by WF(ParentObservesDone) once remaining = 0) and is VIOLATED for   *)
(* the bug (a Panic strands remaining > 0, so ParentObservesDone is never   *)
(* enabled and the system stutters forever with parentDone = FALSE).        *)
Terminating ==
    /\ \A w \in 1..N : wstate[w] = "idle"
    /\ UNCHANGED vars

Next ==
    \/ \E w \in 1..N : Finish(w) \/ Panic(w)
    \/ ParentObservesDone
    \/ Terminating

(* Weak fairness: every running worker eventually acts (Finish or Panic),  *)
(* and the parent eventually observes remaining = 0 once it holds. This is *)
(* the real system (workers always run to a closure exit; the parent polls *)
(* forever). Liveness must therefore hold REGARDLESS of how many workers   *)
(* take the Panic edge.                                                    *)
Fairness ==
    /\ \A w \in 1..N : WF_vars(Finish(w) \/ Panic(w))
    /\ WF_vars(ParentObservesDone)

Spec == Init /\ [][Next]_vars /\ Fairness

(* Safety sanity: done implies the count reached 0 at some point (no       *)
(* spurious done) -- and parentDone implies remaining hit 0.               *)
Inv == (parentDone => (remaining = 0))

(* THE liveness property under test: the collapse always eventually        *)
(* completes. FAILS for FixApplied=FALSE (a Panic strands remaining > 0).  *)
EventuallyDone == <>(parentDone)

=============================================================================
