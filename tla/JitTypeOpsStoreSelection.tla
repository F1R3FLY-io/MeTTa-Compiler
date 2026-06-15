----------------------- MODULE JitTypeOpsStoreSelection -----------------------
(***************************************************************************)
(* JIT get-type store-selection model.                                     *)
(*                                                                         *)
(* jit_runtime_get_type can allocate a type-name atom. The compiled store   *)
(* determines the factory kind: index builds use the active index factory;  *)
(* legacy slab builds use the slab factory. In an index build the JIT arena *)
(* pointer is not a store selector.                                         *)
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

GetTypeFactoryMatchesCompiledStore ==
  FactoryKind = CompiledStore

NoGetTypeSlabFactoryInIndex ==
  CompiledStore = "index" => FactoryKind = "index"

NoGetTypeArenaPtrStoreSelectionInIndex ==
  CompiledStore = "index" => UsesArenaPtr = FALSE

=============================================================================
