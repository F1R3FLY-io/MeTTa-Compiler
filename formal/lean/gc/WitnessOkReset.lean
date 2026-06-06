/-!
Lean model of the cross-cycle witness-ok reset obligation.

`CURRENT_WITNESS_OK` is a non-generational gate. Cycle teardown must clear it
before the next cycle can collect; a later collection is safe only after a fresh
same-cycle witness proof has set the gate again.
-/

namespace MeTTaTron.GC.WitnessOkReset

theorem cleared_witness_ok_blocks_collection
    {WitnessOk Collect : Prop}
    (cleared : Not WitnessOk)
    (gate : Collect -> WitnessOk) :
    Not Collect := by
  intro collect
  exact cleared (gate collect)

theorem fresh_witness_ok_prevents_stale_collect
    {WitnessOk FreshWitness Collect StaleCollect : Prop}
    (gate : Collect -> WitnessOk)
    (fresh : WitnessOk -> FreshWitness)
    (freshNotStale : FreshWitness -> Not StaleCollect) :
    Collect -> Not StaleCollect := by
  intro collect stale
  exact freshNotStale (fresh (gate collect)) stale

end MeTTaTron.GC.WitnessOkReset
