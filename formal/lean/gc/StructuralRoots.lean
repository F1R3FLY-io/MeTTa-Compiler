/-!
Generic CESK structural-root safety theorem.

This file captures the proof obligation used by the MeTTaTron index collector:
roots are the machine registers and persistent anchors, not a discovery
side-channel.  The implementation-side correspondence is checked by the
`roots.rs` machine-equivalence oracles; this theorem states the collector safety
property those roots must realize.
-/

namespace MeTTaTron.GC.StructuralRoots

variable {Addr : Type u}

structure Registers (Addr : Type u) where
  control : Addr -> Prop
  env : Addr -> Prop
  kont : Addr -> Prop
  global : Addr -> Prop

inductive RootSource where
  | structural
  | driver
  | registry

def StructuralRoot (regs : Registers Addr) (a : Addr) : Prop :=
  regs.control a ∨ regs.env a ∨ regs.kont a ∨ regs.global a

def IndexRootSource : RootSource -> Prop
  | RootSource.structural => True
  | RootSource.driver => True
  | RootSource.registry => False

def IndexCollectorRoot
    (Structural Driver : Addr -> Prop)
    (a : Addr) : Prop :=
  Structural a ∨ Driver a

theorem registry_is_not_index_root_source :
    Not (IndexRootSource RootSource.registry) := by
  intro h
  exact h

theorem index_root_source_not_registry
    {source : RootSource}
    (hsource : IndexRootSource source) :
    source ≠ RootSource.registry := by
  intro hregistry
  cases source with
  | structural => cases hregistry
  | driver => cases hregistry
  | registry => exact hsource

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

theorem structural_root_is_reachable
    {regs : Registers Addr} {Edge : Addr -> Addr -> Prop} {a : Addr} :
    StructuralRoot regs a -> Reach (StructuralRoot regs) Edge a := by
  intro hroot
  exact Reach.root hroot

theorem no_future_touch_uaf
    {regs : Registers Addr}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed FutureTouch : Addr -> Prop}
    (markComplete :
      forall {a : Addr}, Reach (StructuralRoot regs) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a))
    (futureTouchesOnlyReachable :
      forall {a : Addr}, FutureTouch a -> Reach (StructuralRoot regs) Edge a) :
    forall {a : Addr}, FutureTouch a -> Not (Freed a) := by
  intro a htouch hfreed
  have hreach := futureTouchesOnlyReachable htouch
  have hmarked := markComplete hreach
  exact sweepOnlyUnmarked hfreed hmarked

theorem registry_independent_no_future_touch_uaf
    {Structural Driver _Registry : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed FutureTouch : Addr -> Prop}
    (markComplete :
      forall {a : Addr}, Reach (IndexCollectorRoot Structural Driver) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a))
    (futureTouchesOnlyIndexReachable :
      forall {a : Addr}, FutureTouch a -> Reach (IndexCollectorRoot Structural Driver) Edge a) :
    forall {a : Addr}, FutureTouch a -> Not (Freed a) := by
  intro a htouch hfreed
  have hreach := futureTouchesOnlyIndexReachable htouch
  have hmarked := markComplete hreach
  exact sweepOnlyUnmarked hfreed hmarked

theorem reachable_node_survives_sweep
    {regs : Registers Addr}
    {Edge : Addr -> Addr -> Prop}
    {Marked Freed : Addr -> Prop}
    (markComplete :
      forall {a : Addr}, Reach (StructuralRoot regs) Edge a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, Reach (StructuralRoot regs) Edge a -> Not (Freed a) := by
  intro a hreach hfreed
  have hmarked := markComplete hreach
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.StructuralRoots
