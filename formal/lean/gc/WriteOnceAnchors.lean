/-!
Write-once global-anchor rooting obligation.

Some E0 roots are process-global write-once anchors. For the bytecode
compiler's cached atoms, the implementation has exactly three
`OnceLock<MettaValue>` cells and `collect_compiler_atom_roots` reads each
initialized cell structurally. The source-coupling harness pins both the reader
and the absence of reset/take deletion paths. This proof captures the abstract
safety shape used by those anchors.
-/

namespace MeTTaTron.GC.WriteOnceAnchors

variable {Anchor : Type u} {Addr : Type v}

inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop where
  | root {a : Addr} : Root a -> Reach Root Edge a
  | step {a b : Addr} : Reach Root Edge a -> Edge a b -> Reach Root Edge b

def WriteOnceAnchorLive
    (Initialized Deleted : Anchor -> Prop)
    (slot : Anchor) : Prop :=
  Initialized slot ∧ ¬ Deleted slot

theorem write_once_anchor_scanned
    {Initialized Deleted Scanned : Anchor -> Prop}
    (notDeleted : ∀ slot, Initialized slot -> ¬ Deleted slot)
    (scanned : ∀ slot, WriteOnceAnchorLive Initialized Deleted slot -> Scanned slot) :
    ∀ slot, Initialized slot -> Scanned slot := by
  intro slot initialized
  exact scanned slot (And.intro initialized (notDeleted slot initialized))

theorem write_once_anchor_rooted
    {Initialized Deleted Scanned : Anchor -> Prop}
    {AnchorValue : Anchor -> Addr -> Prop}
    {StructuralRoot : Addr -> Prop}
    (notDeleted : ∀ slot, Initialized slot -> ¬ Deleted slot)
    (scanned : ∀ slot, WriteOnceAnchorLive Initialized Deleted slot -> Scanned slot)
    (rooted : ∀ slot a, Scanned slot -> AnchorValue slot a -> StructuralRoot a) :
    ∀ slot a,
      Initialized slot ->
      AnchorValue slot a ->
      StructuralRoot a := by
  intro slot a initialized value
  exact rooted slot a (write_once_anchor_scanned notDeleted scanned slot initialized) value

theorem write_once_anchor_survives_collection
    {Initialized Deleted Scanned : Anchor -> Prop}
    {AnchorValue : Anchor -> Addr -> Prop}
    {StructuralRoot Marked Freed : Addr -> Prop}
    {Edge : Addr -> Addr -> Prop}
    (notDeleted : ∀ slot, Initialized slot -> ¬ Deleted slot)
    (scanned : ∀ slot, WriteOnceAnchorLive Initialized Deleted slot -> Scanned slot)
    (rooted : ∀ slot a, Scanned slot -> AnchorValue slot a -> StructuralRoot a)
    (markComplete : ∀ {a}, Reach StructuralRoot Edge a -> Marked a)
    (sweepOnlyUnmarked : ∀ {a}, Freed a -> ¬ Marked a) :
    ∀ slot a,
      Initialized slot ->
      AnchorValue slot a ->
      ¬ Freed a := by
  intro slot a initialized value freed
  have root : StructuralRoot a :=
    write_once_anchor_rooted notDeleted scanned rooted slot a initialized value
  have marked := markComplete (Reach.root root)
  exact sweepOnlyUnmarked freed marked

end MeTTaTron.GC.WriteOnceAnchors
