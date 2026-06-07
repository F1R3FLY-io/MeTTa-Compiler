/-!
Node-edge completeness obligation for the CESK index collector.

The structural-root theorem is only useful if the marker's implementation edge
reader covers every semantic heap edge.  In the index heap those edge classes are
inline handle fields, side-arena SExpr/Conjunction children, and first-class
SpaceHandle contents.  State and Memo nodes are leaves by construction.
-/

namespace MeTTaTron.GC.NodeEdgeCompleteness

variable {Addr : Type u}

def SemanticEdge
    (InlineEdge SideEdge SpaceEdge : Addr -> Addr -> Prop)
    (parent child : Addr) : Prop :=
  InlineEdge parent child ∨ SideEdge parent child ∨ SpaceEdge parent child

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

theorem semantic_edge_covered_by_reader
    {InlineEdge SideEdge SpaceEdge ReaderEdge : Addr -> Addr -> Prop}
    (inlineCovered :
      forall {parent child : Addr}, InlineEdge parent child -> ReaderEdge parent child)
    (sideCovered :
      forall {parent child : Addr}, SideEdge parent child -> ReaderEdge parent child)
    (spaceCovered :
      forall {parent child : Addr}, SpaceEdge parent child -> ReaderEdge parent child) :
    forall {parent child : Addr},
      SemanticEdge InlineEdge SideEdge SpaceEdge parent child ->
      ReaderEdge parent child := by
  intro parent child hedge
  cases hedge with
  | inl hinline => exact inlineCovered hinline
  | inr htail =>
      cases htail with
      | inl hside => exact sideCovered hside
      | inr hspace => exact spaceCovered hspace

theorem semantic_reachable_seen_by_reader
    {Root Seen : Addr -> Prop}
    {InlineEdge SideEdge SpaceEdge ReaderEdge : Addr -> Addr -> Prop}
    (rootSeen : forall {a : Addr}, Root a -> Seen a)
    (readerClosed :
      forall {parent child : Addr}, Seen parent -> ReaderEdge parent child -> Seen child)
    (inlineCovered :
      forall {parent child : Addr}, InlineEdge parent child -> ReaderEdge parent child)
    (sideCovered :
      forall {parent child : Addr}, SideEdge parent child -> ReaderEdge parent child)
    (spaceCovered :
      forall {parent child : Addr}, SpaceEdge parent child -> ReaderEdge parent child) :
    forall {a : Addr},
      Reach Root (SemanticEdge InlineEdge SideEdge SpaceEdge) a ->
      Seen a := by
  intro a hreach
  induction hreach with
  | root hroot => exact rootSeen hroot
  | step hprev hedge ih =>
      exact readerClosed ih
        (semantic_edge_covered_by_reader
          (InlineEdge := InlineEdge)
          (SideEdge := SideEdge)
          (SpaceEdge := SpaceEdge)
          (ReaderEdge := ReaderEdge)
          inlineCovered sideCovered spaceCovered hedge)

theorem semantic_reachable_survives_sweep
    {Root Marked Freed : Addr -> Prop}
    {InlineEdge SideEdge SpaceEdge ReaderEdge : Addr -> Addr -> Prop}
    (rootMarked : forall {a : Addr}, Root a -> Marked a)
    (readerMarkClosed :
      forall {parent child : Addr}, Marked parent -> ReaderEdge parent child -> Marked child)
    (inlineCovered :
      forall {parent child : Addr}, InlineEdge parent child -> ReaderEdge parent child)
    (sideCovered :
      forall {parent child : Addr}, SideEdge parent child -> ReaderEdge parent child)
    (spaceCovered :
      forall {parent child : Addr}, SpaceEdge parent child -> ReaderEdge parent child)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr},
      Reach Root (SemanticEdge InlineEdge SideEdge SpaceEdge) a ->
      Not (Freed a) := by
  intro a hreach hfreed
  have hmarked :=
    semantic_reachable_seen_by_reader
      (Root := Root)
      (Seen := Marked)
      (InlineEdge := InlineEdge)
      (SideEdge := SideEdge)
      (SpaceEdge := SpaceEdge)
      (ReaderEdge := ReaderEdge)
      rootMarked readerMarkClosed inlineCovered sideCovered spaceCovered hreach
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.NodeEdgeCompleteness
