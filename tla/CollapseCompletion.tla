---------------------------- MODULE CollapseCompletion ----------------------------
(***************************************************************************)
(* Liveness model of MeTTaTron's parallel dispatch/collapse completion     *)
(* handshake.                                                             *)
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
(* SECOND BUG CLASS: after the RAII fix, a panic can still leave its result *)
(*   slot unstored while `remaining` reaches 0. Release code must not merge *)
(*   a completed dispatch by filtering that None slot away: that would be a *)
(*   valid-looking but wrong subset. StrictSlotCompletion=TRUE models the   *)
(*   current source rule: parent success requires all slots stored; missing *)
(*   slots produce Error, not successful dispatch/collapse output.          *)
(*                                                                         *)
(* TLC result (see CollapseCompletion.cfg): the temporal property          *)
(*   EventuallyDone == <>(parentDone) is VIOLATED when FixApplied = FALSE   *)
(*   (the panic-then-stuck lasso), and HOLDS when FixApplied = TRUE.        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    N,           \* number of required workers/slots
    FixApplied,  \* TRUE  = CompletionGuard fix (Panic also decrements)
                 \* FALSE = the bug (Panic skips the decrement)
    StrictSlotCompletion
                 \* TRUE  = parent success requires every required slot stored
                 \* FALSE = the bug (completed parent silently drops None slots)

VARIABLES
    remaining,   \* AtomicU32: count of workers that have NOT yet decremented (0..N)
    done,        \* the done flag (set by the worker that drives remaining 1->0)
    wstate,      \* [1..N -> {"running","idle"}] : per-worker liveness state
    slotStored,  \* [1..N -> BOOLEAN] : worker published a result/error slot
    parentDone,  \* the parent's WaitForParallelCollapse arm has observed remaining = 0
    parentOk     \* parent returned successful output, not an Error

vars == <<remaining, done, wstate, slotStored, parentDone, parentOk>>

TypeOK ==
    /\ remaining \in 0..N
    /\ done \in BOOLEAN
    /\ wstate \in [1..N -> {"running", "idle"}]
    /\ slotStored \in [1..N -> BOOLEAN]
    /\ parentDone \in BOOLEAN
    /\ parentOk \in BOOLEAN

Init ==
    /\ remaining = N
    /\ done = FALSE
    /\ wstate = [w \in 1..N |-> "running"]
    /\ slotStored = [w \in 1..N |-> FALSE]
    /\ parentDone = FALSE
    /\ parentOk = FALSE

AllSlotsStored == \A w \in 1..N : slotStored[w]

(* A worker that exits via the NORMAL closure tail: decrement, and the     *)
(* worker that drives remaining 1->0 sets `done`. (eval_loop.rs:3612-3617) *)
Finish(w) ==
    /\ wstate[w] = "running"
    /\ wstate'   = [wstate EXCEPT ![w] = "idle"]
    /\ slotStored' = [slotStored EXCEPT ![w] = TRUE]
    /\ remaining' = remaining - 1
    /\ done'      = (remaining = 1) \/ done
    /\ UNCHANGED <<parentDone, parentOk>>

(* A worker whose eval PANICS. The unwind is swallowed by the pool and the *)
(* worker goes idle. BUG (FixApplied=FALSE): the decrement at :3612 is     *)
(* bypassed. FIX (FixApplied=TRUE): the CompletionGuard's Drop decrements  *)
(* on the unwind edge too.                                                 *)
Panic(w) ==
    /\ wstate[w] = "running"
    /\ wstate'   = [wstate EXCEPT ![w] = "idle"]
    /\ slotStored' = slotStored
    /\ IF FixApplied
         THEN /\ remaining' = remaining - 1
              /\ done'      = (remaining = 1) \/ done
         ELSE /\ remaining' = remaining   \* THE BUG: decrement skipped
              /\ done'      = done
    /\ UNCHANGED <<parentDone, parentOk>>

(* The parent wait arm: required-complete dispatch/collapse exits          *)
(* successfully only when remaining = 0 and all required slots are stored. *)
ParentObservesDone ==
    /\ remaining = 0
    /\ ~parentDone
    /\ parentDone' = TRUE
    /\ parentOk' = IF StrictSlotCompletion THEN AllSlotsStored ELSE TRUE
    /\ UNCHANGED <<remaining, done, wstate, slotStored>>

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

(* Successful dispatch/collapse output may not silently omit an unstored   *)
(* slot. If a slot is missing, the implementation returns Error instead of  *)
(* setting parentOk.                                                        *)
NoSilentSuccessfulDrop == parentOk => AllSlotsStored

(* THE liveness property under test: the dispatch/collapse always          *)
(* completes. FAILS for FixApplied=FALSE (a Panic strands remaining > 0).  *)
EventuallyDone == <>(parentDone)

=============================================================================
