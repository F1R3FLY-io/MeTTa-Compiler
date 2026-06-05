/-!
Lean model of the E1 rendezvous witness root-union obligation.

The live dedicated-GC driver waits on per-thread witness slots via
`requestor_wait_for_all_reified_parked`, then drains `WORKER_ROOT_BUFFER`. This
theorem captures the safety shape of that gate: if every occupied witness slot is
published for the cycle, and publication carries that slot's structural roots into
the buffer, then the drained driver root set contains every occupied participant's
roots.
-/

namespace MeTTaTron.GC.RendezvousWitness

variable {Slot Addr : Type u}

theorem witness_wait_union_complete
    {Occupied Published : Slot -> Prop}
    {SlotRoot : Slot -> Addr -> Prop}
    {BufferRoot DriverRoot : Addr -> Prop}
    (waitAllPublished : forall {s : Slot}, Occupied s -> Published s)
    (publishedRootsBuffered :
      forall {s : Slot} {a : Addr}, Published s -> SlotRoot s a -> BufferRoot a)
    (drainCarriesBuffer : forall {a : Addr}, BufferRoot a -> DriverRoot a) :
    forall {s : Slot} {a : Addr}, Occupied s -> SlotRoot s a -> DriverRoot a := by
  intro s a hoccupied hroot
  have hpublished := waitAllPublished hoccupied
  have hbuffered := publishedRootsBuffered hpublished hroot
  exact drainCarriesBuffer hbuffered

theorem witness_roots_retain_after_mark
    {Occupied Published : Slot -> Prop}
    {SlotRoot : Slot -> Addr -> Prop}
    {BufferRoot DriverRoot Marked Freed : Addr -> Prop}
    (waitAllPublished : forall {s : Slot}, Occupied s -> Published s)
    (publishedRootsBuffered :
      forall {s : Slot} {a : Addr}, Published s -> SlotRoot s a -> BufferRoot a)
    (drainCarriesBuffer : forall {a : Addr}, BufferRoot a -> DriverRoot a)
    (markFromDriverRoots : forall {a : Addr}, DriverRoot a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {s : Slot} {a : Addr}, Occupied s -> SlotRoot s a -> Not (Freed a) := by
  intro s a hoccupied hroot hfreed
  have hdriver :
      DriverRoot a :=
    witness_wait_union_complete
      (Occupied := Occupied)
      (Published := Published)
      (SlotRoot := SlotRoot)
      (BufferRoot := BufferRoot)
      (DriverRoot := DriverRoot)
      waitAllPublished
      publishedRootsBuffered
      drainCarriesBuffer
      hoccupied
      hroot
  have hmarked := markFromDriverRoots hdriver
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.RendezvousWitness
