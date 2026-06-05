/-!
E2 SATB E0 mutation-site obligations.

The SATB proof for concurrent marking assumes every snapshot-live value removed
from a value-bearing E0 substore is available to the marker as a shaded deletion
pre-image. The source-coupling harness pins the concrete categories that realize
this obligation: space-local roots, rule-index roots, and
environment/token/state roots.
-/

namespace MeTTaTron.GC.E0MutationSites

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def SATBRoot
    (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
    (a : Addr) : Prop :=
  InitialRoot a ∨ FinalDriverRoot a ∨ ShadedDeletion a ∨ AllocateBlack a

def E0RemovedPreimage
    (SpacePreimage RulePreimage EnvPreimage : Addr -> Prop)
    (a : Addr) : Prop :=
  SpacePreimage a ∨ RulePreimage a ∨ EnvPreimage a

theorem e0_removed_preimage_shaded
    {SpacePreimage RulePreimage EnvPreimage ShadedDeletion : Addr -> Prop}
    (spaceShaded : forall {a : Addr}, SpacePreimage a -> ShadedDeletion a)
    (ruleShaded : forall {a : Addr}, RulePreimage a -> ShadedDeletion a)
    (envShaded : forall {a : Addr}, EnvPreimage a -> ShadedDeletion a) :
    forall {a : Addr},
      E0RemovedPreimage SpacePreimage RulePreimage EnvPreimage a ->
      ShadedDeletion a := by
  intro a hremoved
  cases hremoved with
  | inl hspace => exact spaceShaded hspace
  | inr hrest =>
      cases hrest with
      | inl hrule => exact ruleShaded hrule
      | inr henv => exact envShaded henv

theorem e0_removed_preimage_is_satb_root
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
      SpacePreimage RulePreimage EnvPreimage : Addr -> Prop}
    (spaceShaded : forall {a : Addr}, SpacePreimage a -> ShadedDeletion a)
    (ruleShaded : forall {a : Addr}, RulePreimage a -> ShadedDeletion a)
    (envShaded : forall {a : Addr}, EnvPreimage a -> ShadedDeletion a) :
    forall {a : Addr},
      E0RemovedPreimage SpacePreimage RulePreimage EnvPreimage a ->
      SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a := by
  intro a hremoved
  exact
    Or.inr
      (Or.inr
        (Or.inl
          (e0_removed_preimage_shaded
            (SpacePreimage := SpacePreimage)
            (RulePreimage := RulePreimage)
            (EnvPreimage := EnvPreimage)
            (ShadedDeletion := ShadedDeletion)
            spaceShaded
            ruleShaded
            envShaded
            hremoved)))

theorem e0_removed_snapshot_live_survives_collection
    {InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
      SpacePreimage RulePreimage EnvPreimage : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed SnapshotLive : Addr -> Prop}
    (spaceShaded : forall {a : Addr}, SpacePreimage a -> ShadedDeletion a)
    (ruleShaded : forall {a : Addr}, RulePreimage a -> ShadedDeletion a)
    (envShaded : forall {a : Addr}, EnvPreimage a -> ShadedDeletion a)
    (removedCoverage :
      forall {a : Addr}, SnapshotLive a -> E0RemovedPreimage SpacePreimage RulePreimage EnvPreimage a)
    (markComplete :
      forall {a : Addr}, Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, SnapshotLive a -> Not (Freed a) := by
  intro a hlive hfreed
  have hroot : SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a :=
    e0_removed_preimage_is_satb_root
      (InitialRoot := InitialRoot)
      (FinalDriverRoot := FinalDriverRoot)
      (ShadedDeletion := ShadedDeletion)
      (AllocateBlack := AllocateBlack)
      (SpacePreimage := SpacePreimage)
      (RulePreimage := RulePreimage)
      (EnvPreimage := EnvPreimage)
      spaceShaded
      ruleShaded
      envShaded
      (removedCoverage hlive)
  have hmarked := markComplete (Reach.root hroot)
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.E0MutationSites
