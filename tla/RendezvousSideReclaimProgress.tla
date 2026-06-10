---- MODULE RendezvousSideReclaimProgress ----
\* Bounded side-payload reclaim progress (#273) under the E1 rendezvous collector.
\*
\* A rendezvous/minor sweep that reclaims an owner slot APPENDS one SideReclaim
\* snapshot (`append_pending_side_reclaims`) but does NOT free the payload Box — it
\* stays committed, deferred. A true-quiescence MAJOR drains the ENTIRE pending vec
\* (`free_pending_side_reclaims` via `mem::take`). `pending_side_major`
\* (`phase == "quiescence" && pending_side_reclaims > 0`) FORCES that major the moment
\* a quiescence point is reached with pending > 0.
\*
\* Discriminator: with the trigger (`PendingSideTrigger = TRUE`) `pending` is drained
\* every quiescence, so it never accumulates past one interval's reclaims
\* (`PendingBounded` HOLDS). Without it (the pre-266d19d rendezvous-only world) a
\* quiescence can be left WITHOUT draining, so `pending` persists across cycles and
\* grows past `MaxReclaims` (`PendingBounded` is VIOLATED). This is the model-checked
\* companion to the deductive Rocq proof RendezvousSideReclaimProgress.v.

EXTENDS Naturals

CONSTANTS PendingSideTrigger,  \* TRUE = `pending_side_major` forces a major at quiescence w/ pending>0
          MaxReclaims,         \* reclaims appendable per rendezvous interval (finite model)
          MaxCycles            \* bound the run length for a finite state space

VARIABLES pending,     \* |pending_side_reclaims|
          this_cycle,  \* reclaims appended in the CURRENT rendezvous interval
          phase        \* "rendezvous" | "quiescence"

Init ==
  /\ pending = 0
  /\ this_cycle = 0
  /\ phase = "rendezvous"

\* A rendezvous sweep reclaims one owner: append a pending snapshot (deferred free).
Reclaim ==
  /\ phase = "rendezvous"
  /\ this_cycle < MaxReclaims
  /\ pending < MaxReclaims * MaxCycles     \* finite-model cap (keeps the state space bounded)
  /\ pending' = pending + 1
  /\ this_cycle' = this_cycle + 1
  /\ UNCHANGED phase

\* Reach a quiescence point (the driver goes idle: active_evaluators in {0,1}).
Quiesce ==
  /\ phase = "rendezvous"
  /\ phase' = "quiescence"
  /\ UNCHANGED <<pending, this_cycle>>

\* The forced major drain (`pending_side_major`): empties the entire pending vec.
Drain ==
  /\ phase = "quiescence"
  /\ PendingSideTrigger
  /\ pending > 0
  /\ pending' = 0
  /\ this_cycle' = 0
  /\ phase' = "rendezvous"

\* Leave quiescence WITHOUT draining. Only enabled when the trigger is OFF (pre-266d19d)
\* or there is nothing pending — exactly the case `pending_side_major` removes.
ResumeNoDrain ==
  /\ phase = "quiescence"
  /\ (~PendingSideTrigger \/ pending = 0)
  /\ this_cycle' = 0
  /\ phase' = "rendezvous"
  /\ UNCHANGED pending

Next == Reclaim \/ Quiesce \/ Drain \/ ResumeNoDrain

Spec == Init /\ [][Next]_<<pending, this_cycle, phase>>

TypeOK ==
  /\ pending \in 0..(MaxReclaims * MaxCycles)
  /\ this_cycle \in 0..MaxReclaims
  /\ phase \in {"rendezvous", "quiescence"}

\* BOUNDED PROGRESS: committed side storage cannot grow unboundedly because `pending`
\* is drained every quiescence — so it never exceeds one interval's reclaims.
PendingBounded == pending <= MaxReclaims

====
