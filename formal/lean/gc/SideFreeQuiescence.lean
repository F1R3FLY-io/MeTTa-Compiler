/-!
Side-payload free quiescence obligation.

Index nodes may materialize laundered references into side-arena boxes. Reclaiming
the fixed node slot is independent from dropping those side boxes: side boxes may
be dropped only on the true-quiescence arm, and the per-thread materialization
shadow must be cleared before any later dereference can observe a dropped box.
-/

namespace MeTTaTron.GC.SideFreeQuiescence

theorem side_free_at_quiescence_has_no_stack_launder_ref
    {Quiescent SideFreed StackLaunderLive : Prop}
    (freeRequiresQuiescence : SideFreed -> Quiescent)
    (stackRefImpliesNonQuiescent : StackLaunderLive -> Not Quiescent) :
    SideFreed -> Not StackLaunderLive := by
  intro hfreed hstack
  exact stackRefImpliesNonQuiescent hstack (freeRequiresQuiescence hfreed)

theorem side_free_shadow_clear_blocks_future_deref
    {Quiescent SideFreed StackLaunderLive ShadowCleared FutureDeref : Prop}
    (freeRequiresQuiescence : SideFreed -> Quiescent)
    (stackRefImpliesNonQuiescent : StackLaunderLive -> Not Quiescent)
    (freeClearsShadowBeforeFutureDeref : SideFreed -> ShadowCleared)
    (futureDerefNeedsLiveRefOrUnclearedShadow :
      FutureDeref -> StackLaunderLive ∨ Not ShadowCleared) :
    SideFreed -> Not FutureDeref := by
  intro hfreed hderef
  have hno_stack :
      Not StackLaunderLive :=
    side_free_at_quiescence_has_no_stack_launder_ref
      freeRequiresQuiescence stackRefImpliesNonQuiescent hfreed
  have hcleared : ShadowCleared := freeClearsShadowBeforeFutureDeref hfreed
  cases futureDerefNeedsLiveRefOrUnclearedShadow hderef with
  | inl hstack => exact hno_stack hstack
  | inr hnot_cleared => exact hnot_cleared hcleared

theorem nonquiescent_collection_defers_side_free
    {Quiescent SideFreed : Prop}
    (freeRequiresQuiescence : SideFreed -> Quiescent) :
    Not Quiescent -> Not SideFreed := by
  intro hnot_quiescent hfreed
  exact hnot_quiescent (freeRequiresQuiescence hfreed)

end MeTTaTron.GC.SideFreeQuiescence
