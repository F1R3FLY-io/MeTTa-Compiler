/-!
Lean model of the single-threaded mid-loop CESK root-union obligation.

The mid-loop collector runs while the trampoline machine is live, so its
root vector must contain the live S/C/K registers, persistent E0 roots, global
anchors, the reified K-spine, deferred environment drops, and the caller-held
driver-C safepoint channel. The source-coupling harness pins the implementation
branch that builds that vector.
-/

namespace MeTTaTron.GC.MidloopRootUnion

variable {Addr : Type u}

def MidloopRootUnion
    (LiveSCK Env0 Global KSpine Deferred DriverC : Addr -> Prop)
    (a : Addr) : Prop :=
  LiveSCK a ∨ Env0 a ∨ Global a ∨ KSpine a ∨ Deferred a ∨ DriverC a

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

theorem midloop_root_union_complete
    {LiveSCK Env0 Global KSpine Deferred DriverC MidloopRoot : Addr -> Prop}
    (liveSCKIncluded : forall {a : Addr}, LiveSCK a -> MidloopRoot a)
    (env0Included : forall {a : Addr}, Env0 a -> MidloopRoot a)
    (globalIncluded : forall {a : Addr}, Global a -> MidloopRoot a)
    (kSpineIncluded : forall {a : Addr}, KSpine a -> MidloopRoot a)
    (deferredIncluded : forall {a : Addr}, Deferred a -> MidloopRoot a)
    (driverCIncluded : forall {a : Addr}, DriverC a -> MidloopRoot a) :
    forall {a : Addr},
      MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC a ->
      MidloopRoot a := by
  intro a hroot
  cases hroot with
  | inl hliveSCK => exact liveSCKIncluded hliveSCK
  | inr rest =>
      cases rest with
      | inl henv0 => exact env0Included henv0
      | inr rest =>
          cases rest with
          | inl hglobal => exact globalIncluded hglobal
          | inr rest =>
              cases rest with
              | inl hkSpine => exact kSpineIncluded hkSpine
              | inr rest =>
                  cases rest with
                  | inl hdeferred => exact deferredIncluded hdeferred
                  | inr hdriverC => exact driverCIncluded hdriverC

theorem midloop_root_union_survives_collection
    {LiveSCK Env0 Global KSpine Deferred DriverC
      MidloopRoot Marked Freed : Addr -> Prop}
    (liveSCKIncluded : forall {a : Addr}, LiveSCK a -> MidloopRoot a)
    (env0Included : forall {a : Addr}, Env0 a -> MidloopRoot a)
    (globalIncluded : forall {a : Addr}, Global a -> MidloopRoot a)
    (kSpineIncluded : forall {a : Addr}, KSpine a -> MidloopRoot a)
    (deferredIncluded : forall {a : Addr}, Deferred a -> MidloopRoot a)
    (driverCIncluded : forall {a : Addr}, DriverC a -> MidloopRoot a)
    (markFromMidloopRoots : forall {a : Addr}, MidloopRoot a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr},
      MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC a ->
      Not (Freed a) := by
  intro a hroot hfreed
  have hmidloop :
      MidloopRoot a :=
    midloop_root_union_complete
      (LiveSCK := LiveSCK)
      (Env0 := Env0)
      (Global := Global)
      (KSpine := KSpine)
      (Deferred := Deferred)
      (DriverC := DriverC)
      (MidloopRoot := MidloopRoot)
      liveSCKIncluded
      env0Included
      globalIncluded
      kSpineIncluded
      deferredIncluded
      driverCIncluded
      hroot
  have hmarked := markFromMidloopRoots hmidloop
  exact sweepOnlyUnmarked hfreed hmarked

theorem midloop_future_touch_survives_collection
    {LiveSCK Env0 Global KSpine Deferred DriverC
      Marked Freed FutureTouch : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    (markComplete :
      forall {a : Addr},
        Reach (MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC) Edge a ->
          Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a))
    (futureTouchesOnlyReachable :
      forall {a : Addr},
        FutureTouch a ->
          Reach (MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC) Edge a) :
    forall {a : Addr}, FutureTouch a -> Not (Freed a) := by
  intro a htouch hfreed
  have hreach := futureTouchesOnlyReachable htouch
  have hmarked := markComplete hreach
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.MidloopRootUnion
