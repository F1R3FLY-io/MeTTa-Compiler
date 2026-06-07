/-!
Frame-local environment roots for forked CESK frames.

Forked nondeterministic environments can carry values in CoW-local maps that
are not reachable from E0. The index collector must therefore read those maps
from every live work item and continuation frame before publishing the mutator's
thread contribution.
-/

namespace MeTTaTron.GC.FrameEnvRoots

variable {Addr : Type u}

def ForkLocalRoot
    (Binding TypeAssertion StateCell NamedSpace InferredType : Addr -> Prop)
    (a : Addr) : Prop :=
  Binding a ∨ TypeAssertion a ∨ StateCell a ∨ NamedSpace a ∨ InferredType a

theorem fork_local_component_in_frame_root
    {Binding TypeAssertion StateCell NamedSpace InferredType FrameRoot : Addr -> Prop}
    (bindingIncluded : forall {a : Addr}, Binding a -> FrameRoot a)
    (typeIncluded : forall {a : Addr}, TypeAssertion a -> FrameRoot a)
    (stateIncluded : forall {a : Addr}, StateCell a -> FrameRoot a)
    (namedSpaceIncluded : forall {a : Addr}, NamedSpace a -> FrameRoot a)
    (inferredIncluded : forall {a : Addr}, InferredType a -> FrameRoot a) :
    forall {a : Addr},
      ForkLocalRoot Binding TypeAssertion StateCell NamedSpace InferredType a ->
      FrameRoot a := by
  intro a hroot
  cases hroot with
  | inl hbinding => exact bindingIncluded hbinding
  | inr hrest =>
      cases hrest with
      | inl htype => exact typeIncluded htype
      | inr hrest =>
          cases hrest with
          | inl hstate => exact stateIncluded hstate
          | inr hrest =>
              cases hrest with
              | inl hnamed => exact namedSpaceIncluded hnamed
              | inr hinferred => exact inferredIncluded hinferred

theorem fork_local_root_survives_collection
    {Binding TypeAssertion StateCell NamedSpace InferredType
      FrameRoot ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop}
    (bindingIncluded : forall {a : Addr}, Binding a -> FrameRoot a)
    (typeIncluded : forall {a : Addr}, TypeAssertion a -> FrameRoot a)
    (stateIncluded : forall {a : Addr}, StateCell a -> FrameRoot a)
    (namedSpaceIncluded : forall {a : Addr}, NamedSpace a -> FrameRoot a)
    (inferredIncluded : forall {a : Addr}, InferredType a -> FrameRoot a)
    (frameIncluded : forall {a : Addr}, FrameRoot a -> ThreadRoot a)
    (publicationBuffersThread :
      forall {a : Addr}, ThreadRoot a -> BufferRoot a)
    (driverDrainsBuffer : forall {a : Addr}, BufferRoot a -> DriverRoot a)
    (markFromDriverRoots : forall {a : Addr}, DriverRoot a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr},
      ForkLocalRoot Binding TypeAssertion StateCell NamedSpace InferredType a ->
      Not (Freed a) := by
  intro a hfork hfreed
  have hframe :
      FrameRoot a :=
    fork_local_component_in_frame_root
      (Binding := Binding)
      (TypeAssertion := TypeAssertion)
      (StateCell := StateCell)
      (NamedSpace := NamedSpace)
      (InferredType := InferredType)
      (FrameRoot := FrameRoot)
      bindingIncluded
      typeIncluded
      stateIncluded
      namedSpaceIncluded
      inferredIncluded
      hfork
  have hthread := frameIncluded hframe
  have hbuffer := publicationBuffersThread hthread
  have hdriver := driverDrainsBuffer hbuffer
  have hmarked := markFromDriverRoots hdriver
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.FrameEnvRoots
