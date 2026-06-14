--------------------- MODULE JitValueCreationStoreSelection ---------------------
(***************************************************************************)
(* JIT value-creation store-selection model.                               *)
(*                                                                         *)
(* The value-creation runtime helpers allocate compound values.  The       *)
(* compiled store determines the factory kind: index builds use the active *)
(* index factory; legacy slab builds use the slab factory.  In an index    *)
(* build the JIT arena pointer is not a store selector.                     *)
(***************************************************************************)

CONSTANTS CompiledStore, FactoryKind, UsesArenaPtr

VARIABLE phase

vars == <<phase>>

Stores == {"index", "slab"}
Factories == {"index", "slab"}

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ CompiledStore \in Stores
  /\ FactoryKind \in Factories
  /\ UsesArenaPtr \in BOOLEAN
  /\ phase = "checked"

FactoryMatchesCompiledStore ==
  FactoryKind = CompiledStore

NoSlabFactoryInIndex ==
  CompiledStore = "index" => FactoryKind = "index"

NoArenaPtrStoreSelectionInIndex ==
  CompiledStore = "index" => UsesArenaPtr = FALSE

=============================================================================
