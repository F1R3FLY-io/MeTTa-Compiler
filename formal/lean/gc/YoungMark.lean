/-!
Lean model of the generational young-only mark obligation.

This is the theorem behind `StoreCentricGC_GenerationalYoungMark.tla` and the
`IndexArena::pop_young_free_slot` cur-segment-only reuse rule:

  * A minor mark visits young roots and young children of already-marked young
    nodes.
  * That is sound when the store has no old-to-young edge.
  * Bump-order allocation/reuse is one sufficient reason no old-to-young edge
    exists after promotion.
  * If the implementation traverses all reachable nodes and only restricts
    which nodes receive a young mark bit, the minor is sound without the
    no-old-to-young premise. This is the obligation used for first-class
    mutable `SpaceHandle` contents.

The proof is parametric in the address type and graph; it is not a finite TLC
state-space check.
-/

namespace MeTTaTron.GC.YoungMark

variable {Addr : Type u}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

theorem bump_order_no_old_to_young
    {seg : Addr -> Nat} {floor : Nat} {Edge : Addr -> Addr -> Prop}
    (bumpOrder : forall {parent child : Addr}, Edge parent child -> seg child <= seg parent) :
    forall {parent child : Addr}, Edge parent child -> floor <= seg child -> floor <= seg parent := by
  intro parent child hedge childYoung
  exact Nat.le_trans childYoung (bumpOrder hedge)

theorem young_reachable_marked
    {Root Young Marked : Addr -> Prop} {Edge : Addr -> Addr -> Prop}
    (youngRootMarked : forall {a : Addr}, Root a -> Young a -> Marked a)
    (noOldToYoung : forall {parent child : Addr}, Edge parent child -> Young child -> Young parent)
    (youngMarkClosed :
      forall {parent child : Addr},
        Marked parent -> Young parent -> Edge parent child -> Young child -> Marked child) :
    forall {a : Addr}, Reach Root Edge a -> Young a -> Marked a := by
  intro a hreach ayoung
  induction hreach with
  | root hroot =>
      exact youngRootMarked hroot ayoung
  | step hparent hedge ih =>
      have parentYoung := noOldToYoung hedge ayoung
      have parentMarked := ih parentYoung
      exact youngMarkClosed parentMarked parentYoung hedge ayoung

def MinorRetains (Young Marked : Addr -> Prop) (a : Addr) : Prop :=
  (Young a -> Marked a)

theorem minor_retains_reachable
    {Root Young Marked : Addr -> Prop} {Edge : Addr -> Addr -> Prop}
    (youngRootMarked : forall {a : Addr}, Root a -> Young a -> Marked a)
    (noOldToYoung : forall {parent child : Addr}, Edge parent child -> Young child -> Young parent)
    (youngMarkClosed :
      forall {parent child : Addr},
        Marked parent -> Young parent -> Edge parent child -> Young child -> Marked child) :
    forall {a : Addr}, Reach Root Edge a -> MinorRetains Young Marked a := by
  intro a hreach ayoung
  exact
    young_reachable_marked
      (Root := Root)
      (Young := Young)
      (Marked := Marked)
      (Edge := Edge)
      youngRootMarked
      noOldToYoung
      youngMarkClosed
      hreach
      ayoung

theorem reachable_seen
    {Root Seen : Addr -> Prop} {Edge : Addr -> Addr -> Prop}
    (rootSeen : forall {a : Addr}, Root a -> Seen a)
    (seenClosed :
      forall {parent child : Addr}, Seen parent -> Edge parent child -> Seen child) :
    forall {a : Addr}, Reach Root Edge a -> Seen a := by
  intro a hreach
  induction hreach with
  | root hroot =>
      exact rootSeen hroot
  | step hparent hedge ih =>
      exact seenClosed ih hedge

theorem conservative_young_reachable_marked
    {Root Young Seen Marked : Addr -> Prop} {Edge : Addr -> Addr -> Prop}
    (rootSeen : forall {a : Addr}, Root a -> Seen a)
    (seenClosed :
      forall {parent child : Addr}, Seen parent -> Edge parent child -> Seen child)
    (seenYoungMarked : forall {a : Addr}, Seen a -> Young a -> Marked a) :
    forall {a : Addr}, Reach Root Edge a -> Young a -> Marked a := by
  intro a hreach ayoung
  have seen : Seen a :=
    reachable_seen
      (Root := Root)
      (Seen := Seen)
      (Edge := Edge)
      rootSeen
      seenClosed
      hreach
  exact seenYoungMarked seen ayoung

theorem conservative_minor_retains_reachable
    {Root Young Seen Marked : Addr -> Prop} {Edge : Addr -> Addr -> Prop}
    (rootSeen : forall {a : Addr}, Root a -> Seen a)
    (seenClosed :
      forall {parent child : Addr}, Seen parent -> Edge parent child -> Seen child)
    (seenYoungMarked : forall {a : Addr}, Seen a -> Young a -> Marked a) :
    forall {a : Addr}, Reach Root Edge a -> MinorRetains Young Marked a := by
  intro a hreach ayoung
  exact
    conservative_young_reachable_marked
      (Root := Root)
      (Young := Young)
      (Seen := Seen)
      (Marked := Marked)
      (Edge := Edge)
      rootSeen
      seenClosed
      seenYoungMarked
      hreach
      ayoung

end MeTTaTron.GC.YoungMark
