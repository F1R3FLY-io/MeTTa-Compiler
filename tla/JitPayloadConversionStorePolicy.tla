----------------------- MODULE JitPayloadConversionStorePolicy -----------------------
(***************************************************************************)
(* Generic JIT payload conversion model.                                    *)
(*                                                                         *)
(* Inline payloads carry their value in the NaN-boxed word and require no   *)
(* heap inspection. Heap/error payloads are decoded according to the         *)
(* compiled store: index builds reconstruct arena handles, while legacy slab *)
(* builds may dereference slab pointers.                                    *)
(***************************************************************************)

CONSTANTS CompiledStore, PayloadKind, DecodeAction

VARIABLE phase

vars == <<phase>>

Stores == {"index", "slab"}
PayloadKinds == {"inline", "heap", "error"}
DecodeActions == {"inline", "index", "slab"}

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ CompiledStore \in Stores
  /\ PayloadKind \in PayloadKinds
  /\ DecodeAction \in DecodeActions
  /\ phase = "checked"

InlinePayloadNoHeapInspect ==
  PayloadKind = "inline" => DecodeAction = "inline"

PayloadDecodeMatchesStore ==
  /\ PayloadKind = "inline" => DecodeAction = "inline"
  /\ PayloadKind /= "inline" => DecodeAction = CompiledStore

NoSlabDerefInIndex ==
  CompiledStore = "index" /\ PayloadKind /= "inline" => DecodeAction = "index"

SlabDerefRequiresSlabStore ==
  DecodeAction = "slab" => CompiledStore = "slab" /\ PayloadKind /= "inline"

=============================================================================
