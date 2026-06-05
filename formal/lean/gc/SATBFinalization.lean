/-!
E2 SATB finalization obligations.

The concurrent SATB path has three finalization duties after background
marking: re-mark final-rendezvous roots before the exclusive sweep, treat a
closed final sweep gate as an abort, and run a freshly requested STW rendezvous
after any abort. The TLA+ models discriminate the interleavings; these Lean
theorems discharge the abstract safety/control obligations that the
source-coupling harness pins to the Rust driver.
-/

namespace MeTTaTron.GC.SATBFinalization

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def SATBRoot
    (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
    (a : Addr) : Prop :=
  InitialRoot a ∨ FinalDriverRoot a ∨ ShadedDeletion a ∨ AllocateBlack a

def RequestHandled (SatbSwept StwFallbackRan : Prop) : Prop :=
  SatbSwept ∨ StwFallbackRan

theorem final_remark_root_is_satb_root
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack FinalRoot : Addr -> Prop}
    (remarkPublished : forall {a : Addr}, FinalRoot a -> FinalDriverRoot a) :
    forall {a : Addr},
      FinalRoot a ->
      SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a := by
  intro a hfinal
  exact Or.inr (Or.inl (remarkPublished hfinal))

theorem final_remark_root_survives_collection
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack FinalRoot : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (remarkPublished : forall {a : Addr}, FinalRoot a -> FinalDriverRoot a)
    (markComplete :
      forall {a : Addr}, Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, FinalRoot a -> Not (Freed a) := by
  intro a hfinal hfreed
  have hroot : SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a :=
    final_remark_root_is_satb_root
      (InitialRoot := InitialRoot)
      (FinalDriverRoot := FinalDriverRoot)
      (ShadedDeletion := ShadedDeletion)
      (AllocateBlack := AllocateBlack)
      (FinalRoot := FinalRoot)
      remarkPublished
      hfinal
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

theorem closed_final_sweep_uses_stw_backstop
    {FinalSweepReturned SatbSwept SatbAbort StwFallbackRan : Prop}
    (closedGateAborts : FinalSweepReturned -> Not SatbSwept -> SatbAbort)
    (abortRunsStw : SatbAbort -> StwFallbackRan) :
    FinalSweepReturned ->
    Not SatbSwept ->
    StwFallbackRan := by
  intro hreturned hnotSwept
  exact abortRunsStw (closedGateAborts hreturned hnotSwept)

theorem aborted_satb_runs_requested_stw
    {SatbAbort StwRequested StwRan : Prop}
    (abortRequestsStw : SatbAbort -> StwRequested)
    (requestedStwRuns : StwRequested -> StwRan) :
    SatbAbort ->
    StwRequested ∧ StwRan := by
  intro habort
  have hrequested := abortRequestsStw habort
  exact And.intro hrequested (requestedStwRuns hrequested)

theorem completed_satb_request_has_collection
    {SatbSuccess SatbAbort SatbSwept StwFallbackRan RequestDone : Prop}
    (successSwept : SatbSuccess -> SatbSwept)
    (abortRunsStw : SatbAbort -> StwFallbackRan)
    (doneCase : RequestDone -> SatbSuccess ∨ SatbAbort) :
    RequestDone ->
    RequestHandled SatbSwept StwFallbackRan := by
  intro hdone
  cases doneCase hdone with
  | inl hsuccess => exact Or.inl (successSwept hsuccess)
  | inr habort => exact Or.inr (abortRunsStw habort)

end MeTTaTron.GC.SATBFinalization
