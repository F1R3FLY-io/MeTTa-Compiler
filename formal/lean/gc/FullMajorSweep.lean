/-!
E2 SATB full-major sweep mark-lifecycle obligations.

The first concurrent SATB path intentionally finishes with a full major sweep.
SATB deletion barriers can mark old addresses; a young-only sweep would leave
those old mark bits stale for a later minor. These theorems capture the abstract
obligation pinned by source coupling: every address that SATB may mark is in the
full-major swept range, and every swept address has its mark cleared before the
next cycle.
-/

namespace MeTTaTron.GC.FullMajorSweep

variable {Addr : Type u}

theorem full_major_clears_all_satb_marks
    {SATBMarked Swept Cleared : Addr -> Prop}
    (satbSwept : forall {a : Addr}, SATBMarked a -> Swept a)
    (sweptCleared : forall {a : Addr}, Swept a -> Cleared a) :
    forall {a : Addr}, SATBMarked a -> Cleared a := by
  intro a hmarked
  exact sweptCleared (satbSwept hmarked)

theorem full_major_leaves_no_stale_mark
    {Swept MarkedAfter : Addr -> Prop}
    (fullSweep : forall {a : Addr}, Swept a)
    (sweptCleared : forall {a : Addr}, Swept a -> Not (MarkedAfter a)) :
    forall {a : Addr}, Not (MarkedAfter a) := by
  intro a hmarkedAfter
  exact sweptCleared fullSweep hmarkedAfter

theorem full_major_then_promotion_preserves_no_stale_mark
    {Swept MarkedAfterSweep MarkedAfterPromotion : Addr -> Prop}
    (fullSweep : forall {a : Addr}, Swept a)
    (sweptCleared : forall {a : Addr}, Swept a -> Not (MarkedAfterSweep a))
    (promotionDoesNotSetMarks : forall {a : Addr}, MarkedAfterPromotion a -> MarkedAfterSweep a) :
    forall {a : Addr}, Not (MarkedAfterPromotion a) := by
  intro a hmarkedAfterPromotion
  exact sweptCleared fullSweep (promotionDoesNotSetMarks hmarkedAfterPromotion)

theorem full_major_satb_marks_not_stale_after_promotion
    {SATBMarked Swept MarkedAfterPromotion : Addr -> Prop}
    (satbSwept : forall {a : Addr}, SATBMarked a -> Swept a)
    (sweptCleared : forall {a : Addr}, Swept a -> Not (MarkedAfterPromotion a)) :
    forall {a : Addr}, SATBMarked a -> Not (MarkedAfterPromotion a) := by
  intro a hsatb hmarkedAfter
  exact sweptCleared (satbSwept hsatb) hmarkedAfter

end MeTTaTron.GC.FullMajorSweep
