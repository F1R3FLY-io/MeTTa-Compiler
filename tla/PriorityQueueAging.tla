--------------------------- MODULE PriorityQueueAging ---------------------------
(***************************************************************************)
(* Discriminator for priority queue aging.                                  *)
(*                                                                         *)
(* RecomputeAtPop = TRUE models refreshing every queued task's score while  *)
(* holding the queue lock immediately before pop. RecomputeAtPop = FALSE    *)
(* models the old enqueue-time score cache: an old low-priority task can    *)
(* age enough to outrank a newer high-priority task, but the heap still     *)
(* pops the newer task.                                                     *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT RecomputeAtPop

VARIABLES oldAge, highEnqueued, popped

vars == <<oldAge, highEnqueued, popped>>

TypeOK ==
    /\ oldAge \in 0..1
    /\ highEnqueued \in BOOLEAN
    /\ popped \in {"none", "old", "high"}

Init ==
    /\ oldAge = 0
    /\ highEnqueued = FALSE
    /\ popped = "none"

AgeOld ==
    /\ oldAge = 0
    /\ oldAge' = 1
    /\ UNCHANGED <<highEnqueued, popped>>

EnqueueHigh ==
    /\ oldAge = 1
    /\ ~highEnqueued
    /\ highEnqueued' = TRUE
    /\ UNCHANGED <<oldAge, popped>>

Pop ==
    /\ highEnqueued
    /\ popped = "none"
    /\ popped' = IF RecomputeAtPop THEN "old" ELSE "high"
    /\ UNCHANGED <<oldAge, highEnqueued>>

Done ==
    /\ popped # "none"
    /\ UNCHANGED vars

Next ==
    \/ AgeOld
    \/ EnqueueHigh
    \/ Pop
    \/ Done

Spec == Init /\ [][Next]_vars

OldPopsAfterAging ==
    /\ highEnqueued
    /\ oldAge = 1
    /\ popped # "none"
    => popped = "old"

=============================================================================
