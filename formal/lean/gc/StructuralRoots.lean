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

def StructuralRoot (regs : Registers Addr) (a : Addr) : Prop :=
  regs.control a ∨ regs.env a ∨ regs.kont a ∨ regs.global a

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
