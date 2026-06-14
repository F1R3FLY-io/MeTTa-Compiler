-------------------------- MODULE JitLongBoxStoreSelection --------------------------
(***************************************************************************)
(* JIT Long boxing store-selection model.                                  *)
(*                                                                         *)
(* Inline Long values do not allocate.  Out-of-inline-range Long values     *)
(* must allocate in the compiled store.  Index builds must not retain a     *)
(* compiled slab fallback after the index branch.                           *)
(***************************************************************************)

CONSTANTS CompiledStore, InlineFits, Allocator, SlabFallbackCompiled

VARIABLE phase

vars == <<phase>>

Stores == {"index", "slab"}
Allocators == {"inline", "index", "slab"}

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ CompiledStore \in Stores
  /\ InlineFits \in BOOLEAN
  /\ Allocator \in Allocators
  /\ SlabFallbackCompiled \in BOOLEAN
  /\ phase = "checked"

InlineLongDoesNotAllocate ==
  InlineFits => Allocator = "inline"

OverflowAllocatorMatchesCompiledStore ==
  ~InlineFits => Allocator = CompiledStore

NoSlabOverflowInIndex ==
  CompiledStore = "index" /\ ~InlineFits => Allocator = "index"

NoSlabFallbackCompiledInIndex ==
  CompiledStore = "index" => SlabFallbackCompiled = FALSE

=============================================================================
