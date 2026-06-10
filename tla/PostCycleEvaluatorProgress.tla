------------------------ MODULE PostCycleEvaluatorProgress ------------------------
(***************************************************************************)
(* Liveness model of the THIRD worker fate under the SATB DOUBLE          *)
(* rendezvous: alive-but-parked-FOREVER (never resumed) across a          *)
(* close -> reopen-SAME-collection cycle boundary.                        *)
(*                                                                         *)
(* #271 e1-default-flip-evaluator-progress-liveness. The now-default      *)
(* dedicated SATB collector path `gc_driver_satb_rendezvous_cycle`         *)
(* (src/backend/eval/cesk/gc_driver.rs:320-356) performs a DOUBLE         *)
(* rendezvous per logical collection:                                     *)
(*                                                                         *)
(*   close_cycle()  [end_rendezvous_cycle bumps GC_CYCLE_GEN K->K+1,      *)
(*                   drops gip#1, resume_workers]                          *)
(*     -> mark_concurrent_roots                                            *)
(*     -> request_next_cycle() [request_gc] + open_cycle(gip#2)            *)
(*     -> prepare_rendezvous_roots() [set_current_cycle_started(K+1)       *)
(*                   notifies the straddle condvar; then waits for the     *)
(*                   witness: every occupied slot stamped/published K+1]   *)
(*     -> sweep_after_concurrent_mark                                      *)
(*     -> close_cycle() [GC_CYCLE_GEN K+1 -> K+2].                         *)
(*                                                                         *)
(* A collapse worker (here the MAIN/PARENT evaluator that holds an         *)
(* EvalGuard while it pump-waits on a collapse worker's `remaining`        *)
(* decrement) parks for rendezvous-1 at generation K, is RESUMED by        *)
(* close_cycle#1, and must RE-PARK for rendezvous-2 across the K->K+1      *)
(* boundary. The re-park happens in the verbatim straddle loop             *)
(* `reacquire_eval_guard_after_safepoint_full` (gc_allocator.rs            *)
(* 6106-6356). Until that parent re-parks-and-publishes for K+1, the       *)
(* driver's witness wait at rdv2 never clears, the sweep never runs,       *)
(* close_cycle#2 never runs, the parent is never resumed, and its          *)
(* CompletionGuard::Drop (which sets `parentDone`) never fires. Permanent  *)
(* hang -- memory-safe, memory-bounded, NOT live.                          *)
(*                                                                         *)
(* This close->reopen-same-collection structure is captured by NONE of    *)
(* the existing liveness models: RendezvousProgress.tla (one posted        *)
(* cycle), CollapseCompletion.tla (worker fates only running->idle),       *)
(* loom_straddle (independent single cycles). This model adds the missing  *)
(* transition.                                                             *)
(*                                                                         *)
(* CONSTANTS (BOOLEAN) parameterize the verified source mechanisms so the  *)
(* established positive/negative discriminator idiom isolates each:        *)
(*                                                                         *)
(*   SatbDoubleRendezvous  TRUE  => model the reopen edge (rdv2) at all.   *)
(*                         FALSE => single rendezvous (no reopen): a       *)
(*                                  sanity control where progress is       *)
(*                                  trivial; not used by the four cfgs.    *)
(*   UseStartedGate        TRUE  => the E5 production gate: re-park only    *)
(*                                  for a cycle a live driver has STARTED   *)
(*                                  (current_cycle_started > my).           *)
(*                         FALSE => the buggy phantom re-park on the bare   *)
(*                                  generation counter (gen > my): the      *)
(*                                  worker parks for K+1 in the teardown/   *)
(*                                  conc-mark window where gen is already   *)
(*                                  K+1 but no driver has committed to K+1. *)
(*   BClosure              TRUE  => the rejoin-tail admission loop re-reads *)
(*                                  `started` and `continue 'straddle`s     *)
(*                                  (republishes) when started advanced;    *)
(*                                  so a terminal break in the teardown     *)
(*                                  window cannot strand the slot           *)
(*                                  occupied-but-unpublished.               *)
(*                         FALSE => the relocated hang: the terminal break  *)
(*                                  leaves published=K while the K+1 driver  *)
(*                                  waits on the occupied,published<started  *)
(*                                  slot forever.                           *)
(*   ResumeNotifyOnReopen  TRUE  => set_current_cycle_started notifies the  *)
(*                                  GC_PROGRESS_CONDVAR straddle waiters on  *)
(*                                  the reopen edge (gc_allocator.rs        *)
(*                                  2982-2990), so a parent waiting in the  *)
(*                                  teardown else-arm wakes when rdv2 opens. *)
(*                         FALSE => the SATB-reopen lost wakeup: the parent  *)
(*                                  sleeps in the else-arm past the K+1      *)
(*                                  start and never re-evaluates the gate.   *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    SatbDoubleRendezvous,
    UseStartedGate,
    BClosure,
    ResumeNotifyOnReopen

VARIABLES
    gen,         \* GC_CYCLE_GEN: 1 = K, 2 = K+1, 3 = K+2.
    started,     \* GC_CYCLE_STARTED: the gen a LIVE driver has committed to.
    gip,         \* GC_IN_PROGRESS: a collector holds the gip guard.
    gcReq,       \* GC_REQUESTED: request_gc posted for the reopen.
    phase,       \* SATB driver phase.
    parkGen,     \* the generation the parent originally parked for (= K = 1).
    parkState,   \* the parent's straddle state.
    slotAcq,     \* witness slot: acquired gen (restamp target).
    slotPub,     \* witness slot: published gen (note_reified_park stamp).
    slotOcc,     \* witness slot: occupied (held from EvalGuard::enter to drop).
    parentDone   \* the parent's CompletionGuard::Drop has fired (resumed + returned).

vars == <<gen, started, gip, gcReq, phase, parkGen, parkState,
          slotAcq, slotPub, slotOcc, parentDone>>

(* phase domain:
     "idle"      = initial snapshot rendezvous held (gip#1, gen=K, started=K).
     "conc_mark" = close_cycle#1 done (gen=K+1, gip=false, workers resumed);
                   mark_concurrent_roots in flight; rdv2 not yet opened.
     "rdv2_wait" = request_next_cycle + open_cycle(gip#2) done (gip=true again),
                   set_current_cycle_started(K+1) done (started=K+1); the driver
                   is in requestor_wait_for_all_reified_parked waiting on the
                   witness (the parent must re-park+publish for K+1).
     "swept"     = sweep_after_concurrent_mark ran (witness was satisfied).
     "swept_bumped" = close_cycle#2's end_rendezvous_cycle bumped GC_CYCLE_GEN
                    K+1->K+2 but the gip#2 guard is NOT yet dropped (the TEARDOWN
                    window: gen=K+2, started=K+1, gip=TRUE). This is the E5
                    StartedCycleGate window: gen names K+2 but no driver has
                    STARTED a K+2 cycle (started stays K+1). A started-gated
                    straddler does NOT re-park here; a gen-gated one phantom-
                    re-parks for K+2 and strands (no K+2 rendezvous ever opens).
     "done"      = close_cycle#2 done (gip#2 dropped, gen=K+2, parent resumable). *)

(* parkState domain:
     "straddling" = in the 'straddle loop, re-evaluating the re-park gate.
     "elsewait"   = parked in the teardown else-arm condvar wait (gip && gen!=my
                    && started<=my): woken only by a reopen notify.
     "republished"= re-parked + published the slot for a started later cycle
                    (witness_restamp_acquired + worker_park_and_root_in_cycle).
     "rejoined"   = broke 'straddle for good and rejoined (terminal break).
     "resumed"    = the parent observed its cycle done and is running again. *)

----------------------------------------------------------------------------
TypeOK ==
    /\ SatbDoubleRendezvous \in BOOLEAN
    /\ UseStartedGate \in BOOLEAN
    /\ BClosure \in BOOLEAN
    /\ ResumeNotifyOnReopen \in BOOLEAN
    /\ gen \in 1..3
    /\ started \in 1..3
    /\ started <= gen
    /\ gip \in BOOLEAN
    /\ gcReq \in BOOLEAN
    /\ phase \in {"idle", "conc_mark", "rdv2_wait", "swept", "swept_bumped", "done"}
    /\ parkGen \in 1..3
    /\ parkState \in {"straddling", "elsewait", "republished", "rejoined", "resumed"}
    /\ slotAcq \in 1..3
    /\ slotPub \in 0..3
    /\ slotOcc \in BOOLEAN
    /\ parentDone \in BOOLEAN

Init ==
    /\ gen = 1
    /\ started = 1
    /\ gip = TRUE
    /\ gcReq = FALSE
    /\ phase = "idle"
    /\ parkGen = 1
    /\ parkState = "straddling"
    /\ slotAcq = 1
    /\ slotPub = 0           \* not yet published for any started cycle.
    /\ slotOcc = TRUE        \* slot occupied from EvalGuard::enter.
    /\ parentDone = FALSE

----------------------------------------------------------------------------
(* ---- DRIVER STEPS: gc_driver.rs:320-356 ---- *)

(* close_cycle#1: end_rendezvous_cycle bumps gen K->K+1, drops gip#1,           *)
(* resume_workers. The parent's rendezvous-1 park is released here; it now       *)
(* runs the 'straddle loop.                                                      *)
DriverClose1 ==
    /\ phase = "idle"
    /\ gen = 1
    /\ phase' = "conc_mark"
    /\ gen' = 2
    /\ gip' = FALSE
    /\ UNCHANGED <<started, gcReq, parkGen, parkState,
                   slotAcq, slotPub, slotOcc, parentDone>>

(* request_next_cycle() [request_gc -> gcReq] + open_cycle(gip#2) [gip true].    *)
(* gen stays K+1; started still K until SetStarted2.                             *)
DriverOpen2 ==
    /\ SatbDoubleRendezvous
    /\ phase = "conc_mark"
    /\ phase' = "rdv2_wait"
    /\ gcReq' = TRUE
    /\ gip' = TRUE
    /\ UNCHANGED <<gen, started, parkGen, parkState,
                   slotAcq, slotPub, slotOcc, parentDone>>

(* set_current_cycle_started(K+1) inside prepare_rendezvous_roots: started=K+1   *)
(* (Release store) AND notify the straddle condvar IFF ResumeNotifyOnReopen.     *)
(* A parent waiting in the "elsewait" else-arm is woken back to "straddling"     *)
(* only when the notify fires (Mesa re-check). gen already = K+1 here.           *)
DriverSetStarted2 ==
    /\ phase = "rdv2_wait"
    /\ started < gen
    /\ started' = gen
    /\ parkState' = IF (ResumeNotifyOnReopen /\ parkState = "elsewait")
                      THEN "straddling"
                      ELSE parkState
    /\ UNCHANGED <<gen, gip, gcReq, phase, parkGen,
                   slotAcq, slotPub, slotOcc, parentDone>>

(* sweep_after_concurrent_mark: fires ONLY when the witness is satisfied --       *)
(* every occupied slot was published for the started cycle (slotPub >= started).  *)
(* This is requestor_wait_for_all_reified_parked + the gate_open_rendezvous       *)
(* witness predicate. If the slot is occupied but published < started the driver  *)
(* is BLOCKED here (no enabled step) -- the witness wait.                          *)
DriverSweep ==
    /\ phase = "rdv2_wait"
    /\ started = gen                       \* SetStarted2 has run (started = K+1).
    /\ (slotOcc => slotPub >= started)     \* THE WITNESS: occupied => published.
    /\ phase' = "swept"
    /\ UNCHANGED <<gen, started, gip, gcReq, parkGen, parkState,
                   slotAcq, slotPub, slotOcc, parentDone>>

(* close_cycle#2 = close_open_rendezvous_cycle, modeled in its TWO real steps to    *)
(* expose the E5 teardown window:                                                   *)
(*   (i)  end_rendezvous_cycle: bump GC_CYCLE_GEN K+1->K+2 (started STAYS K+1, gip  *)
(*        STILL held). gen now NAMES K+2 but no driver has started a K+2 cycle.     *)
(*   (ii) drop(gip) + resume_workers: gip#2 drops, the cycle is fully done.         *)
(* The window between (i) and (ii) is exactly StartedCycleGate.tla's EndCycle/      *)
(* DropGip window. A gen-gated straddler that is still parked (republished for      *)
(* K+1) re-evaluates here: gen(K+2) > parkGen(K+1) is true, so it phantom-re-parks  *)
(* for K+2 -- a cycle that NEVER gets a started rendezvous (the collection is       *)
(* ending) -- and waits for gen to advance past K+2, which never happens. The       *)
(* started-gate (started=K+1 <= parkGen=K+1) does NOT re-park here, so the parent   *)
(* proceeds to resume.                                                              *)
DriverBumpGen2 ==
    /\ phase = "swept"
    /\ phase' = "swept_bumped"
    /\ gen' = 3
    /\ UNCHANGED <<started, gip, gcReq, parkGen, parkState,
                   slotAcq, slotPub, slotOcc, parentDone>>

DriverDropGip2 ==
    /\ phase = "swept_bumped"
    /\ phase' = "done"
    /\ gip' = FALSE
    /\ gcReq' = FALSE
    /\ UNCHANGED <<gen, started, parkGen, parkState,
                   slotAcq, slotPub, slotOcc, parentDone>>

----------------------------------------------------------------------------
(* ---- THE STRADDLE: reacquire_eval_guard_after_safepoint_full (the verbatim   *)
(*      3-way branch, parameterized by UseStartedGate / BClosure) ---- *)

(* The re-park gate predicate. UseStartedGate=TRUE keys on the STARTED cycle     *)
(* (E5 production); FALSE keys on the bare generation counter (the buggy         *)
(* phantom re-park). Compared against parkGen (= my_reparked_gen).               *)
ShouldRepark ==
    IF UseStartedGate
      THEN started > parkGen
      ELSE gen > parkGen

(* (a) A REAL (or, under the buggy gate, phantom) later cycle needs this thread: *)
(* witness_restamp_acquired(target) + worker_park_and_root_in_cycle(target).     *)
(* worker_park_and_root_in_cycle publishes (note_reified_park) ONLY if           *)
(* GC_CYCLE_GEN == target -- i.e. only for the LIVE cycle. Under UseStartedGate  *)
(* the target = started <= gen, and since the only later started cycle is K+1     *)
(* with gen=K+1, the publish lands. Under the buggy gen-gate the target = gen,    *)
(* so the publish also lands, but it can fire in the conc_mark window BEFORE any  *)
(* driver committed to that gen -- a phantom that the witness wait at rdv2 never  *)
(* needed and that strands the parent.                                           *)
StraddleRepark ==
    /\ parkState \in {"straddling"}
    /\ ShouldRepark
    /\ LET target == IF UseStartedGate THEN started ELSE gen
       IN /\ slotAcq' = target
          /\ slotPub' = (IF gen = target THEN target ELSE slotPub)
          /\ parkGen' = target
          /\ parkState' = "republished"
    /\ slotOcc' = TRUE
    /\ UNCHANGED <<gen, started, gip, gcReq, phase, parentDone>>

(* A republished parent stays parked for its target cycle until that cycle ENDS  *)
(* (gen advances past it). Then it loops back to 'straddle to re-evaluate.        *)
StraddleRepublishWait ==
    /\ parkState = "republished"
    /\ gen > parkGen
    /\ parkState' = "straddling"
    /\ UNCHANGED <<gen, started, gip, gcReq, phase, parkGen,
                   slotAcq, slotPub, slotOcc, parentDone>>

(* (c) Teardown else-arm: gip && gen != my && started <= my. The parent waits on *)
(* GC_PROGRESS_CONDVAR. It is woken ONLY by a reopen notify (DriverSetStarted2    *)
(* when ResumeNotifyOnReopen) or by the gip-clear (DriverClose2). Modeled as a    *)
(* transition INTO "elsewait"; the wake transitions are folded into               *)
(* DriverSetStarted2 (-> "straddling") and the gip-drop re-enabling the gate.      *)
StraddleElseWait ==
    /\ parkState = "straddling"
    /\ ~ShouldRepark
    /\ gip
    /\ gen # parkGen
    /\ started <= parkGen
    /\ parkState' = "elsewait"
    /\ UNCHANGED <<gen, started, gip, gcReq, phase, parkGen,
                   slotAcq, slotPub, slotOcc, parentDone>>

(* The gip-clear path out of the else-arm: when gip drops (Close2), the else-arm  *)
(* predicate (gip && started<=my) is false, so the wait returns and the parent    *)
(* loops back to 'straddle. (DriverClose2 sets gip=FALSE; this re-enables the      *)
(* gate.) Independent of the reopen notify.                                       *)
StraddleElseWake ==
    /\ parkState = "elsewait"
    /\ ~gip
    /\ parkState' = "straddling"
    /\ UNCHANGED <<gen, started, gip, gcReq, phase, parkGen,
                   slotAcq, slotPub, slotOcc, parentDone>>

(* Terminal break toward the rejoin tail: started <= my && !gip (no started cycle *)
(* waits on this thread). In the SATB teardown window this can fire with stale     *)
(* started=K just after Close1 dropped gip#1 and BEFORE Open2/SetStarted2.         *)
(* With BClosure the rejoin tail re-reads `started` and bounces back; without it,  *)
(* the parent rejoins for good at the stale gen and the K+1 driver waits forever   *)
(* on its occupied,published<started slot.                                         *)
StraddleTerminalBreak ==
    /\ parkState = "straddling"
    /\ ~ShouldRepark
    /\ ~gip
    /\ parkState' = "rejoined"
    /\ UNCHANGED <<gen, started, gip, gcReq, phase, parkGen,
                   slotAcq, slotPub, slotOcc, parentDone>>

(* The rejoin-tail admission loop. BClosure=TRUE: if started advanced past my     *)
(* reparked gen, go BACK into 'straddle (republish for the started cycle) instead *)
(* of sitting in a non-publishing wait. BClosure=FALSE: the parent has already    *)
(* rejoined for good -- no re-check -- and stays "rejoined" while the driver waits *)
(* on its unpublished slot. (When started <= my, the rejoin completes and the      *)
(* parent is effectively parked awaiting its own resume edge, Close2.)             *)
RejoinBClosureBounce ==
    /\ parkState = "rejoined"
    /\ BClosure
    /\ started > parkGen
    /\ parkState' = "straddling"
    /\ UNCHANGED <<gen, started, gip, gcReq, phase, parkGen,
                   slotAcq, slotPub, slotOcc, parentDone>>

----------------------------------------------------------------------------
(* ---- PARENT RESUME + COMPLETION ---- *)

(* The parent observes its collection complete (Close2: gen=K+2, gip=false) and    *)
(* its straddle/rejoin gate lets it through (no started cycle later than its         *)
(* reparked gen is in flight): it resumes. ONLY from "straddling"/"rejoined" --      *)
(* a "republished" parent is BLOCKED INSIDE worker_park_and_root_in_cycle, waiting  *)
(* for its target cycle to END (worker_resume_wait_for_cycle: gen != parkGen). It    *)
(* does NOT poll the resume gate; only StraddleRepublishWait (its cycle ends) can     *)
(* release it. This is the crux of the gen-gate strand: a gen-gated phantom re-park   *)
(* for the TERMINAL gen K+2 enters an in-cycle wait for a K+2-end that never comes.   *)
ParentResume ==
    /\ phase = "done"
    /\ ~gip
    /\ ~ShouldRepark
    /\ parkState \in {"straddling", "rejoined"}
    /\ parkState' = "resumed"
    /\ slotOcc' = FALSE          \* outermost EvalGuard::drop releases the slot.
    /\ UNCHANGED <<gen, started, gip, gcReq, phase, parkGen,
                   slotAcq, slotPub, parentDone>>

(* CompletionGuard::Drop fires on the resumed parent's return: parentDone. This   *)
(* is the collapse `remaining`->0 + `done` the pump observes (CollapseCompletion). *)
ParentObservesDone ==
    /\ parkState = "resumed"
    /\ ~parentDone
    /\ parentDone' = TRUE
    /\ UNCHANGED <<gen, started, gip, gcReq, phase, parkGen, parkState,
                   slotAcq, slotPub, slotOcc>>

----------------------------------------------------------------------------
(* Stutter once the parent is done OR the system is stuck, so a terminal/stuck    *)
(* state is not flagged as a (safety) deadlock -- the LIVENESS property            *)
(* EventuallyParentDone is then the discriminator.                                 *)
Stutter == UNCHANGED vars

Next ==
    \/ DriverClose1
    \/ DriverOpen2
    \/ DriverSetStarted2
    \/ DriverSweep
    \/ DriverBumpGen2
    \/ DriverDropGip2
    \/ StraddleRepark
    \/ StraddleRepublishWait
    \/ StraddleElseWait
    \/ StraddleElseWake
    \/ StraddleTerminalBreak
    \/ RejoinBClosureBounce
    \/ ParentResume
    \/ ParentObservesDone
    \/ Stutter

(* Weak fairness on the driver steps, the straddle steps, and the parent's        *)
(* observe-done. Matches the real system: the dedicated GC thread always advances  *)
(* its phases; a runnable straddling parent always re-evaluates its gate; the      *)
(* parent always eventually observes done once resumed. The LIVENESS property      *)
(* must therefore hold REGARDLESS of scheduling -- the only thing that can strand  *)
(* the parent is a missing transition (a lost wakeup, a missing B-closure, or a    *)
(* phantom re-park), NOT an unfair schedule.                                        *)
Fairness ==
    /\ WF_vars(DriverClose1)
    /\ WF_vars(DriverOpen2)
    /\ WF_vars(DriverSetStarted2)
    /\ WF_vars(DriverSweep)
    /\ WF_vars(DriverBumpGen2)
    /\ WF_vars(DriverDropGip2)
    /\ WF_vars(StraddleRepark)
    /\ WF_vars(StraddleRepublishWait)
    /\ WF_vars(StraddleElseWait)
    /\ WF_vars(StraddleElseWake)
    /\ WF_vars(StraddleTerminalBreak)
    /\ WF_vars(RejoinBClosureBounce)
    /\ WF_vars(ParentResume)
    /\ WF_vars(ParentObservesDone)

Spec == Init /\ [][Next]_vars /\ Fairness

----------------------------------------------------------------------------
(* ---- PROPERTIES ---- *)

(* PRIMARY liveness: the parent evaluator always eventually completes. FAILS for  *)
(* the reopen lost-wakeup, the missing B-closure, and the phantom (gen) gate;      *)
(* HOLDS for the all-TRUE fix.                                                      *)
EventuallyParentDone == <>(parentDone)

(* COMPANION: the stranded predicate cannot persist. "parentActiveButParked" = the *)
(* parent holds its EvalGuard (slot occupied, not resumed) while gc is idle (no     *)
(* gip) and it is parked in the straddle. Whenever that holds, it must eventually   *)
(* clear. This is the abstract shape of the SIGUSR1 dump: active_evaluators=1,      *)
(* gc_in_progress=false, all worker pools sleeping, output partial.                 *)
parentActiveButParked ==
    /\ slotOcc
    /\ ~parentDone
    /\ parkState \in {"straddling", "elsewait", "republished", "rejoined"}
gcIdle == ~gip
workersParked == parkState \in {"elsewait", "republished"}

StrandedCannotPersist ==
    [] ( (parentActiveButParked /\ gcIdle /\ workersParked)
            => <>(~parentActiveButParked) )

(* SAFETY co-assertion (checked as an INVARIANT in ALL four cfgs, including the     *)
(* bug ones): the sweep never runs while an occupied witness slot is published for  *)
(* an EARLIER cycle than the started one. This proves the FIX does not "avoid the   *)
(* deadlock" by skipping the sweep on an under-rooted heap (which would be a UAF):  *)
(* progress is achieved by the parent actually re-parking+publishing, never by      *)
(* sweeping past an unpublished live slot. It holds VACUOUSLY in the bug cfgs        *)
(* (the sweep is never reached), so the discriminator between fix and bug is purely  *)
(* the LIVENESS property while SAFETY is universal.                                  *)
NoSweepWhileUnpublished ==
    (phase \in {"swept", "swept_bumped", "done"}) => ~(slotOcc /\ slotPub < started)

=============================================================================
