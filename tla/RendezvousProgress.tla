---------------------------- MODULE RendezvousProgress ----------------------------
(***************************************************************************)
(* Dedicated CESK-GC rendezvous progress model.                            *)
(*                                                                         *)
(* This model combines the previously separate progress obligations that   *)
(* matter to the occasional deadlock class: a posted rendezvous request,   *)
(* every active participant contributing by park/finish, collection cleanup*)
(* closing the cycle even on panic, generation-based worker resume, and    *)
(* request clearing / notify at teardown. It does not prove general thread *)
(* pool fairness; it assumes weak fairness for enabled worker/driver steps.*)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    N,
    WorkersContribute,
    CleanupOnPanic,
    BumpAtClose,
    UseGenerationResume,
    ResumeNotify,
    BackToBackBeforeResume

VARIABLES
    phase,
    wstate,
    gen,
    myGen,
    gcRequested,
    backToBackRequested,
    closedByCleanup

vars == <<phase, wstate, gen, myGen, gcRequested, backToBackRequested,
          closedByCleanup>>

TypeOK ==
    /\ N \in Nat
    /\ N > 0
    /\ WorkersContribute \in BOOLEAN
    /\ CleanupOnPanic \in BOOLEAN
    /\ BumpAtClose \in BOOLEAN
    /\ UseGenerationResume \in BOOLEAN
    /\ ResumeNotify \in BOOLEAN
    /\ BackToBackBeforeResume \in BOOLEAN
    /\ phase \in {"start", "waiting", "collecting", "closed", "done", "stuck"}
    /\ wstate \in [1..N -> {"active", "parked", "resumed"}]
    /\ gen \in 1..2
    /\ myGen \in 1..2
    /\ gcRequested \in BOOLEAN
    /\ backToBackRequested \in BOOLEAN
    /\ closedByCleanup \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ wstate = [w \in 1..N |-> "active"]
    /\ gen = 1
    /\ myGen = 1
    /\ gcRequested = FALSE
    /\ backToBackRequested = FALSE
    /\ closedByCleanup = FALSE

RequestPosted ==
    /\ phase = "start"
    /\ phase' = "waiting"
    /\ gcRequested' = TRUE
    /\ UNCHANGED <<wstate, gen, myGen, backToBackRequested, closedByCleanup>>

WorkerContributes(w) ==
    /\ phase = "waiting"
    /\ WorkersContribute
    /\ wstate[w] = "active"
    /\ wstate' = [wstate EXCEPT ![w] = "parked"]
    /\ UNCHANGED <<phase, gen, myGen, gcRequested, backToBackRequested,
                  closedByCleanup>>

AllContributed ==
    \A w \in 1..N : wstate[w] # "active"

WaitComplete ==
    /\ phase = "waiting"
    /\ AllContributed
    /\ phase' = "collecting"
    /\ UNCHANGED <<wstate, gen, myGen, gcRequested, backToBackRequested,
                  closedByCleanup>>

CloseCycle ==
    /\ phase' = "closed"
    /\ gen' = IF BumpAtClose THEN 2 ELSE gen
    /\ gcRequested' = FALSE
    /\ closedByCleanup' = TRUE
    /\ UNCHANGED <<wstate, myGen, backToBackRequested>>

CollectionReturns ==
    /\ phase = "collecting"
    /\ CloseCycle

CollectionPanics ==
    /\ phase = "collecting"
    /\ IF CleanupOnPanic
       THEN CloseCycle
       ELSE
          /\ phase' = "stuck"
          /\ UNCHANGED <<wstate, gen, myGen, gcRequested, backToBackRequested,
                        closedByCleanup>>

BackToBackRequest ==
    /\ phase = "closed"
    /\ BackToBackBeforeResume
    /\ ~backToBackRequested
    /\ gcRequested' = TRUE
    /\ backToBackRequested' = TRUE
    /\ UNCHANGED <<phase, wstate, gen, myGen, closedByCleanup>>

CanResume ==
    IF UseGenerationResume
      THEN gen # myGen
      ELSE ~gcRequested

WorkerResume(w) ==
    /\ phase = "closed"
    /\ ResumeNotify
    /\ wstate[w] = "parked"
    /\ CanResume
    /\ wstate' = [wstate EXCEPT ![w] = "resumed"]
    /\ UNCHANGED <<phase, gen, myGen, gcRequested, backToBackRequested,
                  closedByCleanup>>

AllResumed ==
    \A w \in 1..N : wstate[w] = "resumed"

DoneStep ==
    /\ phase = "closed"
    /\ AllResumed
    /\ phase' = "done"
    /\ UNCHANGED <<wstate, gen, myGen, gcRequested, backToBackRequested,
                  closedByCleanup>>

Stutter ==
    UNCHANGED vars

Next ==
    \/ RequestPosted
    \/ \E w \in 1..N : WorkerContributes(w)
    \/ WaitComplete
    \/ CollectionReturns
    \/ CollectionPanics
    \/ BackToBackRequest
    \/ \E w \in 1..N : WorkerResume(w)
    \/ DoneStep
    \/ Stutter

Fairness ==
    /\ WF_vars(RequestPosted)
    /\ \A w \in 1..N : WF_vars(WorkerContributes(w))
    /\ WF_vars(WaitComplete)
    /\ WF_vars(CollectionReturns \/ CollectionPanics)
    /\ WF_vars(BackToBackRequest)
    /\ \A w \in 1..N : WF_vars(WorkerResume(w))
    /\ WF_vars(DoneStep)

Spec == Init /\ [][Next]_vars /\ Fairness

NoDoneWithParkedWorker ==
    phase = "done" => AllResumed

EventuallyRendezvousResumed ==
    <>(phase = "done")

=============================================================================
