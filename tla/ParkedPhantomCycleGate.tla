----------------------- MODULE ParkedPhantomCycleGate -----------------------
(***************************************************************************)
(* Bug #309 — the phantom-future park (the branch-B/pump park path's twin  *)
(* of the E5 straddle phantom fixed by StartedCycleGate).                  *)
(*                                                                         *)
(* A worker that observed `is_gc_requested()` reads                        *)
(* `my_gen = current_cycle_gen()` on its way to park. If the driver CLOSES *)
(* the cycle in that window (end-bump K -> K+1, request cleared), the      *)
(* worker reads my_gen = K+1 and parks "for cycle K+1" — a cycle nobody    *)
(* requested. Inside worker_park_and_root_in_cycle the straggler gate      *)
(* `GC_CYCLE_GEN == my_gen` PASSES (gen IS K+1), so the phantom publishes  *)
(* roots, BUMPS THE PARKED COUNT for the unrequested cycle (a masked-      *)
(* parker hazard for a later real K+1), and strands in                     *)
(* worker_resume_wait_for_cycle (waiting for gen -> K+2 forever: captured  *)
(* live at rep 17 of the autopsy farm — cycle_gen 95, cycle_started 94,    *)
(* gc_requested FALSE, gc_wait park = 2).                                  *)
(*                                                                         *)
(* THE GATE (RealCycleGate = TRUE): a worker may publish/bump/park for     *)
(* my_gen only while the cycle is REAL: started = my_gen (already opened)  *)
(* \/ requested (about to open). A phantom (started < my_gen /\ ~requested)*)
(* must skip the park entirely and resume evaluating.                      *)
(***************************************************************************)

EXTENDS Naturals

CONSTANTS RealCycleGate          \* TRUE = the fixed protocol

VARIABLES
    gen,        \* GC_CYCLE_GEN (bumped at cycle close)
    started,    \* current_cycle_started (set when a cycle opens)
    requested,  \* GC_REQUESTED
    worker,     \* "evaluating" | "parking" | "parked" | "resumed"
    myGen,      \* the gen the worker captured on its way to park
    phantomBump \* TRUE once a phantom bumped the parked count (the hazard)

vars == <<gen, started, requested, worker, myGen, phantomBump>>

TypeOK ==
    /\ gen \in 0..3
    /\ started \in 0..3
    /\ requested \in BOOLEAN
    /\ worker \in {"evaluating", "parking", "parked", "resumed"}
    /\ myGen \in 0..3
    /\ phantomBump \in BOOLEAN

Init ==
    /\ gen = 1 /\ started = 0          \* cycle 0 closed; none open
    /\ requested = TRUE                \* cycle 1 has been requested
    /\ worker = "evaluating"
    /\ myGen = 0
    /\ phantomBump = FALSE

(* The worker saw the request and heads to park, capturing the CURRENT gen. *)
WorkerCapture ==
    /\ worker = "evaluating"
    /\ worker' = "parking"
    /\ myGen' = gen
    /\ UNCHANGED <<gen, started, requested, phantomBump>>

(* The driver opens the requested cycle (set_current_cycle_started). *)
DriverOpen ==
    /\ requested /\ started < gen
    /\ started' = gen
    /\ UNCHANGED <<gen, requested, worker, myGen, phantomBump>>

(* The driver closes the open cycle: end-bump + clear the request + wake.
   A parked worker of THIS cycle resumes (gen moves past its myGen). *)
DriverClose ==
    /\ started = gen /\ gen < 3
    /\ gen' = gen + 1
    /\ requested' = FALSE
    /\ worker' = IF worker = "parked" /\ myGen = gen THEN "resumed" ELSE worker
    /\ UNCHANGED <<started, myGen, phantomBump>>

(* The park attempt (worker_park_and_root_in_cycle): the straggler gate
   `gen = myGen` plus — iff RealCycleGate — the started/requested reality
   check. A phantom that parks bumps the count for an unrequested cycle. *)
WorkerPark ==
    /\ worker = "parking"
    /\ IF gen = myGen /\ (~RealCycleGate \/ started = myGen \/ requested)
         THEN /\ worker' = "parked"
              /\ phantomBump' = (phantomBump \/ (started < myGen /\ ~requested))
         ELSE /\ worker' = "resumed"     \* stale or phantom: skip the park
              /\ phantomBump' = phantomBump
    /\ UNCHANGED <<gen, started, requested, myGen>>

(* Terminal stutter: the resumed worker (or the gen-domain cap) is a valid
   end state — the self-loop satisfies TLC's deadlock checker (the canonical
   idiom; see CollapseCompletion.tla). *)
Terminating ==
    /\ worker = "resumed" \/ gen = 3
    /\ UNCHANGED vars

Next ==
    \/ WorkerCapture
    \/ DriverOpen
    \/ DriverClose
    \/ WorkerPark
    \/ Terminating

Spec ==
    Init /\ [][Next]_vars
         /\ WF_vars(DriverOpen) /\ WF_vars(DriverClose) /\ WF_vars(WorkerPark)

(* LIVENESS (leads-to): a worker that PARKS always eventually resumes.
   The BUG config (RealCycleGate = FALSE) violates this via the phantom
   trace: DriverOpen, DriverClose (gen 1 -> 2, request cleared),
   WorkerCapture (myGen = 2), WorkerPark (gen = myGen = 2 passes the
   straggler gate; started = 1 < 2, ~requested) -> parked forever (nothing
   is requested, so no DriverOpen/Close ever bumps gen past 2). *)
ParkedEventuallyResumes == (worker = "parked") ~> (worker = "resumed")

(* SAFETY: a phantom never bumps the parked count (the masked-parker /
   missed-root hazard for a later real cycle). *)
NoPhantomCountBump == ~phantomBump

=============================================================================
