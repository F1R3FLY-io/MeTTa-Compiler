/-!
Hash-cons sweep-retain obligation for the index heap.

The ground-SExpr hash-cons table stores arena handles. A later lookup may return
one of those handles, so sweep must drop entries whose address is about to be
reclaimed. Major sweep retains only marked entries. Minor sweep retains old
entries unconditionally and young entries only when marked, matching the
young-only sweep range.
-/

namespace MeTTaTron.GC.HashConsSweepRetain

variable {Addr : Type u}

theorem major_hash_cons_hit_not_freed
    {Retained Marked Freed Returned : Addr -> Prop}
    (returnedRetained : forall {a : Addr}, Returned a -> Retained a)
    (retainedMarked : forall {a : Addr}, Retained a -> Marked a)
    (sweepFreesOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, Returned a -> Not (Freed a) := by
  intro a hreturned hfreed
  exact sweepFreesOnlyUnmarked hfreed (retainedMarked (returnedRetained hreturned))

theorem minor_hash_cons_hit_not_freed
    {Retained Young Marked Freed Returned : Addr -> Prop}
    (returnedRetained : forall {a : Addr}, Returned a -> Retained a)
    (retainedOldOrMarked :
      forall {a : Addr}, Retained a -> Not (Young a) ∨ Marked a)
    (minorFreesYoungUnmarked :
      forall {a : Addr}, Freed a -> Young a ∧ Not (Marked a)) :
    forall {a : Addr}, Returned a -> Not (Freed a) := by
  intro a hreturned hfreed
  have hretained := returnedRetained hreturned
  have hfreed_shape := minorFreesYoungUnmarked hfreed
  cases retainedOldOrMarked hretained with
  | inl hold =>
      exact hold hfreed_shape.left
  | inr hmarked =>
      exact hfreed_shape.right hmarked

end MeTTaTron.GC.HashConsSweepRetain
