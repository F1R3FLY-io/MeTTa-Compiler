/-!
Lean model of the E5 started-cycle straddle gate obligation.

`GC_CYCLE_GEN` is bumped during teardown before a new driver has committed to
the next cycle. A straddling worker must therefore re-park only when
`GC_CYCLE_STARTED` has advanced beyond its last park generation.
-/

namespace MeTTaTron.GC.StartedCycleGate

theorem started_gate_prevents_phantom_repark
    {StartedAfterMy Repark Phantom : Prop}
    (gate : Repark -> StartedAfterMy)
    (phantomReparks : Phantom -> Repark)
    (phantomStale : Phantom -> Not StartedAfterMy) :
    Not Phantom := by
  intro phantom
  exact phantomStale phantom (gate (phantomReparks phantom))

theorem real_started_cycle_repark_is_not_phantom
    {StartedAfterMy Repark Phantom : Prop}
    (reparkWhenStarted : StartedAfterMy -> Repark)
    (phantomStale : Phantom -> Not StartedAfterMy) :
    StartedAfterMy -> Repark ∧ Not Phantom := by
  intro started
  exact And.intro (reparkWhenStarted started) (fun phantom => phantomStale phantom started)

end MeTTaTron.GC.StartedCycleGate
