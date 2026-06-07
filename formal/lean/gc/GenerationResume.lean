/-!
Lean model of the E1/E5 generation-gated worker resume obligation.

A worker parked for rendezvous generation `myGen` may resume exactly when
`GC_CYCLE_GEN` has advanced. This makes resume independent of `GC_REQUESTED`,
so a back-to-back request cannot re-block a worker whose own cycle ended.
-/

namespace MeTTaTron.GC.GenerationResume

def CanResume (myGen currentGen : Nat) : Prop :=
  currentGen ≠ myGen

def BooleanCanResume (gcRequested : Prop) : Prop :=
  Not gcRequested

theorem generation_end_bump_releases_worker
    {myGen currentGen : Nat}
    (advanced : currentGen ≠ myGen) :
    CanResume myGen currentGen := by
  exact advanced

theorem back_to_back_request_does_not_block_generation_resume
    {myGen currentGen : Nat} {GcRequested : Prop}
    (advanced : currentGen ≠ myGen) :
    GcRequested -> CanResume myGen currentGen := by
  intro _
  exact advanced

theorem same_generation_keeps_worker_parked
    {myGen currentGen : Nat}
    (same : currentGen = myGen) :
    Not (CanResume myGen currentGen) := by
  intro canResume
  exact canResume same

theorem boolean_resume_reasserted_request_blocks
    {GcRequested : Prop}
    (requested : GcRequested) :
    Not (BooleanCanResume GcRequested) := by
  intro canResume
  exact canResume requested

end MeTTaTron.GC.GenerationResume
