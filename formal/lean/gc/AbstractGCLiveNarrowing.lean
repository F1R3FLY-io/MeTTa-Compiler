/-!
C2 abstract-GC live-field narrowing.

`Continuation.collect_live_values` narrows the K register by omitting three
post-cut iterator fields. The narrowing is sound only when every value a future
transition can touch is a full K-frame root and is not one of the dead fields.
-/

namespace MeTTaTron.GC.AbstractGCLiveNarrowing

inductive NarrowedFrame where
  | processRuleMatches
  | processAmb
  | processMatchTemplates

def AbstractRoot
    {Addr : Type u}
    (FullRoot DeadForNext : Addr -> Prop)
    (a : Addr) : Prop :=
  FullRoot a ∧ ¬ DeadForNext a

theorem narrowed_roots_subset_full
    {Addr : Type u}
    {FullRoot DeadForNext : Addr -> Prop} :
    ∀ {a : Addr}, AbstractRoot FullRoot DeadForNext a -> FullRoot a := by
  intro a hroot
  exact hroot.left

theorem no_dead_field_narrowing_equals_full
    {Addr : Type u}
    {FullRoot DeadForNext : Addr -> Prop}
    (noDead : ∀ a, ¬ DeadForNext a) :
    ∀ {a : Addr}, FullRoot a -> AbstractRoot FullRoot DeadForNext a := by
  intro a hfull
  exact And.intro hfull (noDead a)

theorem future_touch_is_abstract_root
    {Addr : Type u}
    {FullRoot DeadForNext FutureTouch : Addr -> Prop}
    (futureInFull : ∀ {a : Addr}, FutureTouch a -> FullRoot a)
    (futureNotDead : ∀ {a : Addr}, FutureTouch a -> ¬ DeadForNext a) :
    ∀ {a : Addr}, FutureTouch a -> AbstractRoot FullRoot DeadForNext a := by
  intro a htouch
  exact And.intro (futureInFull htouch) (futureNotDead htouch)

theorem future_touch_survives_live_narrowing
    {Addr : Type u}
    {FullRoot DeadForNext FutureTouch Marked Freed : Addr -> Prop}
    (futureInFull : ∀ {a : Addr}, FutureTouch a -> FullRoot a)
    (futureNotDead : ∀ {a : Addr}, FutureTouch a -> ¬ DeadForNext a)
    (markLiveRoots :
      ∀ {a : Addr}, AbstractRoot FullRoot DeadForNext a -> Marked a)
    (sweepOnlyUnmarked : ∀ {a : Addr}, Freed a -> ¬ Marked a) :
    ∀ {a : Addr}, FutureTouch a -> ¬ Freed a := by
  intro a htouch hfreed
  have hroot :
      AbstractRoot FullRoot DeadForNext a :=
    future_touch_is_abstract_root
      (FullRoot := FullRoot)
      (DeadForNext := DeadForNext)
      (FutureTouch := FutureTouch)
      futureInFull
      futureNotDead
      htouch
  have hmarked := markLiveRoots hroot
  exact sweepOnlyUnmarked hfreed hmarked

theorem skipped_live_field_is_not_justified
    {Addr : Type u}
    {DeadForNext FutureTouch : Addr -> Prop}
    {a : Addr}
    (htouch : FutureTouch a)
    (futureDead : DeadForNext a)
    (futureNotDead : ∀ {b : Addr}, FutureTouch b -> ¬ DeadForNext b) :
    False := by
  exact futureNotDead htouch futureDead

end MeTTaTron.GC.AbstractGCLiveNarrowing
