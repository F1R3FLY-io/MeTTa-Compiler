--------------------------- MODULE RholangBatchCompletion ---------------------------
(***************************************************************************)
(* Async Rholang batch completion model.                                    *)
(*                                                                         *)
(* `evaluate_batch_parallel_arena` waits until every spawned batch worker   *)
(* has decremented a shared `remaining` counter.  CompletionGuard = FALSE   *)
(* models the tempting tail-decrement implementation: a worker panic skips  *)
(* the decrement, the pool swallows the unwind, and the async caller waits  *)
(* forever. CompletionGuard = TRUE models an RAII guard whose Drop runs on  *)
(* normal and panic-unwind exits.                                           *)
(*                                                                         *)
(* StrictSlotCompletion = TRUE models the required no-silent-drop rule:     *)
(* completion with a missing slot is not a successful smaller batch.        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    N,
    CompletionGuard,
    StrictSlotCompletion

VARIABLES
    remaining,
    done,
    wstate,
    slotStored,
    parentDone,
    parentOk

vars == <<remaining, done, wstate, slotStored, parentDone, parentOk>>

TypeOK ==
    /\ N \in Nat
    /\ remaining \in 0..N
    /\ done \in BOOLEAN
    /\ wstate \in [1..N -> {"running", "idle"}]
    /\ slotStored \in [1..N -> BOOLEAN]
    /\ parentDone \in BOOLEAN
    /\ parentOk \in BOOLEAN
    /\ CompletionGuard \in BOOLEAN
    /\ StrictSlotCompletion \in BOOLEAN

Init ==
    /\ remaining = N
    /\ done = FALSE
    /\ wstate = [w \in 1..N |-> "running"]
    /\ slotStored = [w \in 1..N |-> FALSE]
    /\ parentDone = FALSE
    /\ parentOk = FALSE

AllSlotsStored ==
    \A w \in 1..N : slotStored[w]

Finish(w) ==
    /\ wstate[w] = "running"
    /\ wstate' = [wstate EXCEPT ![w] = "idle"]
    /\ slotStored' = [slotStored EXCEPT ![w] = TRUE]
    /\ remaining' = remaining - 1
    /\ done' = (done \/ (remaining = 1))
    /\ UNCHANGED <<parentDone, parentOk>>

Panic(w) ==
    /\ wstate[w] = "running"
    /\ wstate' = [wstate EXCEPT ![w] = "idle"]
    /\ slotStored' = slotStored
    /\ IF CompletionGuard
       THEN /\ remaining' = remaining - 1
            /\ done' = (done \/ (remaining = 1))
       ELSE /\ remaining' = remaining
            /\ done' = done
    /\ UNCHANGED <<parentDone, parentOk>>

ParentObservesDone ==
    /\ done
    /\ ~parentDone
    /\ parentDone' = TRUE
    /\ parentOk' = IF StrictSlotCompletion THEN AllSlotsStored ELSE TRUE
    /\ UNCHANGED <<remaining, done, wstate, slotStored>>

Terminating ==
    /\ \A w \in 1..N : wstate[w] = "idle"
    /\ UNCHANGED vars

Next ==
    \/ \E w \in 1..N : Finish(w) \/ Panic(w)
    \/ ParentObservesDone
    \/ Terminating

Fairness ==
    /\ \A w \in 1..N : WF_vars(Finish(w) \/ Panic(w))
    /\ WF_vars(ParentObservesDone)

Spec == Init /\ [][Next]_vars /\ Fairness

DoneMeansCountZero ==
    done => remaining = 0

ParentDoneMeansCountZero ==
    parentDone => remaining = 0

NoSilentBatchSuccess ==
    parentOk => AllSlotsStored

EventuallyParentDone ==
    <>(parentDone)

=============================================================================
