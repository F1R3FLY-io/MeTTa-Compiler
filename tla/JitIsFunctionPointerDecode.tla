------------------------- MODULE JitIsFunctionPointerDecode -------------------------
(***************************************************************************)
(* JIT is-function TAG_PTR decode model.                                    *)
(*                                                                         *)
(* TAG_PTR payloads are decoded according to the compiled store.  Index     *)
(* builds reconstruct an arena handle; legacy slab builds may dereference a *)
(* slab pointer.  Non-pointer values require no heap inspection.            *)
(***************************************************************************)

CONSTANTS CompiledStore, TagPtr, Inspection

VARIABLE phase

vars == <<phase>>

Stores == {"index", "slab"}
Inspections == {"none", "index", "slab"}

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ CompiledStore \in Stores
  /\ TagPtr \in BOOLEAN
  /\ Inspection \in Inspections
  /\ phase = "checked"

NonPointerNoInspect ==
  ~TagPtr => Inspection = "none"

PointerInspectionMatchesStore ==
  TagPtr => Inspection = CompiledStore

NoSlabDerefInIndex ==
  CompiledStore = "index" /\ TagPtr => Inspection = "index"

SlabDerefRequiresSlabPointer ==
  Inspection = "slab" => CompiledStore = "slab" /\ TagPtr

=============================================================================
