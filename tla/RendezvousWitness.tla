---------------------------- MODULE RendezvousWitness ----------------------------
(***************************************************************************)
(* E1 rendezvous witness predicate model.                                  *)
(*                                                                         *)
(* Load-bearing source invariant: a sweep may proceed only after every     *)
(* OCCUPIED witness slot satisfies                                         *)
(*                                                                         *)
(*   published_gen >= cur_gen OR acquired_gen > cur_gen                    *)
(*                                                                         *)
(* The strict ">" is essential. A slot with acquired_gen = cur_gen and     *)
(* published_gen < cur_gen has re-stamped for the current cycle but has    *)
(* not yet published its structural roots into WORKER_ROOT_BUFFER.         *)
(* If the predicate is weakened to acquired_gen >= cur_gen, the driver can *)
(* sweep while that slot's roots are not buffered.                         *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Slots,
    StrictPredicate,
    FinisherStamps

CurGen == 1

VARIABLES
    occupied,
    acquired,
    published,
    buffered,
    swept

vars == <<occupied, acquired, published, buffered, swept>>

TypeOK ==
    /\ occupied \in [Slots -> BOOLEAN]
    /\ acquired \in [Slots -> 0..2]
    /\ published \in [Slots -> 0..1]
    /\ buffered \in [Slots -> BOOLEAN]
    /\ swept \in BOOLEAN
    /\ FinisherStamps \in BOOLEAN

Init ==
    /\ occupied = [s \in Slots |-> FALSE]
    /\ acquired = [s \in Slots |-> 0]
    /\ published = [s \in Slots |-> 0]
    /\ buffered = [s \in Slots |-> FALSE]
    /\ swept = FALSE

(* A current-cycle entrant/re-entrant has acquired the current generation but
   has not yet published roots. This is the dangerous state. *)
AcquireCurrent(s) ==
    /\ ~swept
    /\ ~occupied[s]
    /\ occupied' = [occupied EXCEPT ![s] = TRUE]
    /\ acquired' = [acquired EXCEPT ![s] = CurGen]
    /\ published' = [published EXCEPT ![s] = 0]
    /\ buffered' = [buffered EXCEPT ![s] = FALSE]
    /\ UNCHANGED swept

(* A post-snapshot entrant is excluded from the current cycle by acquired>cur. *)
AcquireFuture(s) ==
    /\ ~swept
    /\ ~occupied[s]
    /\ occupied' = [occupied EXCEPT ![s] = TRUE]
    /\ acquired' = [acquired EXCEPT ![s] = CurGen + 1]
    /\ published' = [published EXCEPT ![s] = 0]
    /\ buffered' = [buffered EXCEPT ![s] = FALSE]
    /\ UNCHANGED swept

(* Genuine reified park: roots are buffered before published is stamped. *)
PublishCurrent(s) ==
    /\ ~swept
    /\ occupied[s]
    /\ acquired[s] = CurGen
    /\ buffered' = [buffered EXCEPT ![s] = TRUE]
    /\ published' = [published EXCEPT ![s] = CurGen]
    /\ UNCHANGED <<occupied, acquired, swept>>

(* Non-reified finisher bump: it may publish result roots elsewhere, but it is
   not a reified park of this slot's complete machine. It must therefore NOT
   stamp published for the witness. *)
FinishBump(s) ==
    /\ ~swept
    /\ occupied[s]
    /\ acquired[s] = CurGen
    /\ published' = [published EXCEPT ![s] =
        IF FinisherStamps THEN CurGen ELSE published[s]]
    /\ UNCHANGED <<occupied, acquired, buffered, swept>>

Release(s) ==
    /\ ~swept
    /\ occupied[s]
    /\ occupied' = [occupied EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<acquired, published, buffered, swept>>

SlotSatisfied(s) ==
    published[s] >= CurGen \/
    IF StrictPredicate
      THEN acquired[s] > CurGen
      ELSE acquired[s] >= CurGen

CanSweep ==
    \A s \in Slots : ~occupied[s] \/ SlotSatisfied(s)

Sweep ==
    /\ CanSweep
    /\ ~swept
    /\ swept' = TRUE
    /\ UNCHANGED <<occupied, acquired, published, buffered>>

Done ==
    /\ swept
    /\ UNCHANGED vars

Next ==
    \/ \E s \in Slots :
        AcquireCurrent(s) \/ AcquireFuture(s) \/ PublishCurrent(s) \/ FinishBump(s) \/ Release(s)
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

(* If the driver sweeps, every occupied slot whose acquisition belongs to the
   current-or-earlier cycle has published roots in the driver buffer. *)
RootCompleteOnSweep ==
    swept => \A s \in Slots : occupied[s] /\ acquired[s] <= CurGen => buffered[s]

=============================================================================
