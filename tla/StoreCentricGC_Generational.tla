------------------- MODULE StoreCentricGC_Generational -------------------
(*****************************************************************************)
(* TLA+ model of the GENERATIONAL minor/major collector — Phase C, C1.b of   *)
(* the genuine-CESK GC migration. Companion to StoreCentricGC.tla.           *)
(*                                                                           *)
(* StoreCentricGC.tla verified the NON-MOVING, structural-Ψ, true-quiescence *)
(* FULL collector (its 5 safety invariants + liveness). C1.b adds a          *)
(* GENERATIONAL layer ON TOP of that collector: segments form a young/old    *)
(* boundary `youngFloor` (segments >= youngFloor are YOUNG), a MINOR sweep    *)
(* reclaims only young dead, and a MAJOR sweep reclaims everywhere; after     *)
(* either, PROMOTE reclassifies the swept segments as old (non-moving — a     *)
(* pure boundary advance).                                                   *)
(*                                                                           *)
(* This model isolates the GENERATIONAL question (does the minor preserve     *)
(* memory safety?) from the rendezvous question (already discharged by the   *)
(* base): it models a SINGLE mutator at QUIESCENCE — mutate*, then mark, then *)
(* sweep — because C1.b runs the collector at exactly the base's proven       *)
(* single-threaded quiescence point and changes only the SWEEP, never the     *)
(* rendezvous. So `phase` cycles mutating -> marking -> sweeping -> mutating  *)
(* with no concurrent mutation during mark/sweep (the base's                  *)
(* QuiescenceInvariant holds by the same argument, not re-modeled here).      *)
(*                                                                           *)
(* THE C1.b DESIGN CLAIMS THIS MODEL DISCHARGES:                             *)
(*                                                                           *)
(*   (A) The mark is FULL in BOTH paths (conservative-complete). The minor    *)
(*       does NOT use a young-only mark, so it does NOT rely on a             *)
(*       "no old->young edge" theorem. We DELIBERATELY model old->young       *)
(*       edges as reachable (RewireEdge lets any live node point at any       *)
(*       occupied node, younger or older — exactly the edge that free-list    *)
(*       reuse of an old slot can create in the implementation). The minor    *)
(*       must remain sound in their presence. (This is why C1.b uses the      *)
(*       full mark; the young-only mark of C1.c would be UNSOUND here — see    *)
(*       MinorVsYoungOnlyMark below, a counterexample-by-construction.)       *)
(*                                                                           *)
(*   (B) A MINOR reclaims ONLY young slots (MinorSweepOnlyReclaimsYoung) and   *)
(*       NEVER a reachable slot (NoUseAfterFree, with MinorSweep in Next).     *)
(*                                                                           *)
(*   (C) After a full mark, every reachable node is marked (NoLostObjects),    *)
(*       so neither sweep reclaims a reachable node.                          *)
(*                                                                           *)
(*   (D) A released (young) segment holds no reachable node                   *)
(*       (SegmentReleaseSafety).                                             *)
(*****************************************************************************)

EXTENDS Integers, FiniteSets

CONSTANTS
    SEGMENTS,       \* Fixed finite set of segment ids, e.g. {0,1}
    OFFSETS,        \* Fixed finite set of offsets per segment, e.g. {0,1}
    MaxRoots        \* Upper bound on |psi| (keeps the state space finite)

Addr == SEGMENTS \X OFFSETS
SegOf(a) == a[1]
AddrsInSeg(s) == {a \in Addr : SegOf(a) = s}

\* Segment ids are integers (the high bits of an Addr). The young/old boundary
\* `youngFloor` ranges over segment ids 0..MaxSeg plus a "nothing is young"
\* sentinel MaxSeg+1. We work with SEGMENTS = 0..N-1.
MaxSeg == CHOOSE s \in SEGMENTS : \A t \in SEGMENTS : t <= s

VARIABLES
    store,              \* Addr -> {"free","live","marked"}
    psi,                \* SUBSET Addr — the single structural root set
    edges,              \* Addr -> SUBSET Addr — child handles (any occupied, incl younger)
    youngFloor,         \* segment id: segments with SegOf >= youngFloor are YOUNG
    phase,              \* {"mutating","marking","sweeping"}
    freeList,           \* SUBSET Addr reclaimed by the last sweep
    releasedSegments    \* SUBSET SEGMENTS released wholesale (all-dead) by a sweep

vars == <<store, psi, edges, youngFloor, phase, freeList, releasedSegments>>

IsYoung(a) == SegOf(a) >= youngFloor
IsOld(a)   == SegOf(a) <  youngFloor

(*---------------------------------------------------------------------------*)
(* Reachability — transitive closure of psi over edges, among occupied nodes  *)
(* (identical technique to the base spec).                                    *)
(*---------------------------------------------------------------------------*)
OccupiedAddrs(st) == {a \in Addr : st[a] \in {"live","marked"}}

ExpandOnce(R, st) ==
    R \union UNION { edges[a] : a \in {x \in R : st[x] \in {"live","marked"}} }

RECURSIVE ClosureFuel(_, _, _)
ClosureFuel(R, st, fuel) ==
    IF fuel <= 0 THEN R
    ELSE LET R2 == ExpandOnce(R, st)
         IN IF R2 = R THEN R ELSE ClosureFuel(R2, st, fuel - 1)

ReachableUnder(roots, st) == ClosureFuel(roots, st, Cardinality(Addr))

Reachable == { a \in ReachableUnder(psi, store) : store[a] \in {"live","marked"} }

Occupied == OccupiedAddrs(store)
FreeAddrs == {a \in Addr : store[a] = "free" /\ SegOf(a) \notin releasedSegments}
CanAllocate == FreeAddrs /= {}

(*===========================================================================*)
(* Initial State                                                             *)
(*===========================================================================*)
Init ==
    /\ store = [a \in Addr |-> "free"]
    /\ psi = {}
    /\ edges = [a \in Addr |-> {}]
    /\ youngFloor = 0                 \* all segments young initially
    /\ phase = "mutating"
    /\ freeList = {}
    /\ releasedSegments = {}

(*===========================================================================*)
(* MUTATOR ACTIONS (phase = "mutating"). A single mutator at quiescence.      *)
(*===========================================================================*)

\* Alloc: pop a free slot (free-list reuse OR fresh bump — modeled as ANY free
\* Addr, OLD or YOUNG). Crucially the chosen slot MAY be OLD: this is the
\* free-list-reuse-of-an-old-slot case that, combined with RewireEdge pointing
\* it at younger children, creates a genuine OLD->YOUNG edge. The new node is
\* live and touched by the machine state (a root).
Alloc ==
    /\ phase = "mutating"
    /\ Cardinality(psi) < MaxRoots
    /\ CanAllocate
    /\ \E a \in FreeAddrs :
        /\ store' = [store EXCEPT ![a] = "live"]
        /\ freeList' = freeList \ {a}
        /\ edges' = [edges EXCEPT ![a] = {}]
        /\ psi' = psi \union {a}
    /\ UNCHANGED <<youngFloor, phase, releasedSegments>>

AddRoot ==
    /\ phase = "mutating"
    /\ Cardinality(psi) < MaxRoots
    /\ \E a \in Addr :
        /\ store[a] \in {"live","marked"}
        /\ a \notin psi
        /\ psi' = psi \union {a}
    /\ UNCHANGED <<store, edges, youngFloor, phase, freeList, releasedSegments>>

RemoveRoot ==
    /\ phase = "mutating"
    /\ psi /= {}
    /\ \E a \in psi : psi' = psi \ {a}
    /\ UNCHANGED <<store, edges, youngFloor, phase, freeList, releasedSegments>>

\* RewireEdge: point a live node's handles at ANY occupied nodes — younger or
\* older. This is what makes OLD->YOUNG edges reachable in the model (an old
\* parent, e.g. a free-list-reused old slot, linking freshly-young children).
RewireEdge ==
    /\ phase = "mutating"
    /\ \E a \in Addr :
        /\ store[a] \in {"live","marked"}
        /\ \E newChildren \in SUBSET Occupied :
            /\ edges' = [edges EXCEPT ![a] = newChildren]
    /\ UNCHANGED <<store, psi, youngFloor, phase, freeList, releasedSegments>>

(*===========================================================================*)
(* COLLECTOR — FULL mark (both paths), then a MINOR or MAJOR sweep + promote. *)
(*===========================================================================*)

BeginMark ==
    /\ phase = "mutating"
    /\ phase' = "marking"
    /\ UNCHANGED <<store, psi, edges, youngFloor, freeList, releasedSegments>>

\* FULL transitive mark from psi over edges (C1.b marks the full reachable set
\* in BOTH the minor and the major path — this is the heart of why the minor is
\* sound without a no-old->young theorem).
MarkStep ==
    /\ phase = "marking"
    /\ \E a \in Addr :
        /\ store[a] = "live"
        /\ \/ a \in psi
           \/ \E p \in Addr : store[p] = "marked" /\ a \in edges[p]
        /\ store' = [store EXCEPT ![a] = "marked"]
    /\ UNCHANGED <<psi, edges, youngFloor, phase, freeList, releasedSegments>>

MarkComplete ==
    /\ phase = "marking"
    /\ ~(\E a \in Addr :
            /\ store[a] = "live"
            /\ \/ a \in psi
               \/ \E p \in Addr : store[p] = "marked" /\ a \in edges[p])
    /\ phase' = "sweeping"
    /\ UNCHANGED <<store, psi, edges, youngFloor, freeList, releasedSegments>>

\* Promote target: any segment id >= the current youngFloor (a non-moving
\* boundary advance — the implementation uses cur_seg; we over-approximate by
\* letting TLC explore EVERY legal advance, so safety is proven for all
\* promotion choices). youngFloor never decreases.
PromoteTargets == {f \in 0..(MaxSeg + 1) : f >= youngFloor}

\* MAJOR sweep: reclaim ALL unmarked occupied, clear all marks, rebuild the free
\* list, release fully-dead segments, promote. (= the base Sweep + promotion.)
MajorSweep ==
    /\ phase = "sweeping"
    /\ \E nf \in PromoteTargets :
        LET newStore == [a \in Addr |->
                           CASE store[a] = "marked" -> "live"
                             [] store[a] = "live"   -> "free"
                             [] OTHER               -> "free"]
            deadSegs == {s \in SEGMENTS : \A a \in AddrsInSeg(s) : newStore[a] = "free"}
            newRel   == releasedSegments \union deadSegs
        IN /\ store' = newStore
           /\ freeList' = {a \in Addr : newStore[a] = "free" /\ SegOf(a) \notin newRel}
           /\ releasedSegments' = newRel
           /\ edges' = [a \in Addr |-> IF newStore[a] = "free" THEN {} ELSE edges[a]]
           /\ youngFloor' = nf
           /\ phase' = "mutating"
    /\ UNCHANGED <<psi>>

\* MINOR sweep: reclaim ONLY young unmarked occupied; OLD slots are left exactly
\* as they are (an old unmarked "live" node is floating garbage retained until a
\* major — NOT reclaimed by the minor). Marks are cleared (a faithful
\* abstraction: the impl leaves stale OLD marks set, harmless since the next
\* full mark re-marks; clearing here keeps NoMarksWhileMutating and does not
\* affect any safety property). Only fully-dead YOUNG segments may be released.
\* Then promote.
MinorSweep ==
    /\ phase = "sweeping"
    /\ \E nf \in PromoteTargets :
        LET newStore == [a \in Addr |->
                           CASE store[a] = "marked"             -> "live"      \* survivor (young or old) -> clear mark
                             [] store[a] = "live" /\ IsYoung(a) -> "free"      \* YOUNG unmarked -> reclaim
                             [] store[a] = "live"               -> "live"      \* OLD unmarked -> RETAINED (defer to major)
                             [] OTHER                           -> "free"]     \* already free
            \* Only YOUNG segments can be released by a minor (old never touched).
            deadYoungSegs == {s \in SEGMENTS :
                                /\ s >= youngFloor
                                /\ \A a \in AddrsInSeg(s) : newStore[a] = "free"}
            newRel == releasedSegments \union deadYoungSegs
        IN /\ store' = newStore
           \* Free list grows by reclaimed YOUNG slots; old free-list entries from
           \* a prior major are retained (NOT cleared — sweep_young's clear=false).
           /\ freeList' = (freeList \union {a \in Addr : newStore[a] = "free"})
                            \ {a \in Addr : SegOf(a) \in newRel}
           /\ releasedSegments' = newRel
           /\ edges' = [a \in Addr |-> IF newStore[a] = "free" THEN {} ELSE edges[a]]
           /\ youngFloor' = nf
           /\ phase' = "mutating"
    /\ UNCHANGED <<psi>>

Next ==
    \/ Alloc
    \/ AddRoot
    \/ RemoveRoot
    \/ RewireEdge
    \/ BeginMark
    \/ MarkStep
    \/ MarkComplete
    \/ MajorSweep
    \/ MinorSweep

Spec == Init /\ [][Next]_vars

(*===========================================================================*)
(* TYPE CORRECTNESS                                                           *)
(*===========================================================================*)
TypeOK ==
    /\ store \in [Addr -> {"free","live","marked"}]
    /\ psi \subseteq Addr
    /\ edges \in [Addr -> SUBSET Addr]
    /\ youngFloor \in 0..(MaxSeg + 1)
    /\ phase \in {"mutating","marking","sweeping"}
    /\ freeList \subseteq Addr
    /\ releasedSegments \subseteq SEGMENTS

(*===========================================================================*)
(* SAFETY INVARIANTS                                                         *)
(*===========================================================================*)

\* (1) NoUseAfterFree — THE memory-safety property, now checked with the MINOR
\* sweep in Next: no reachable Addr (young or old) is ever "free". If a minor
\* (or major) reclaimed a reachable slot, this fails.
NoUseAfterFree == \A a \in Reachable : store[a] /= "free"

\* (2) NoLostObjects — after a full mark, every reachable node is marked.
NoLostObjects == (phase = "sweeping") => (\A a \in Reachable : store[a] = "marked")

\* (3) SegmentReleaseSafety — a released segment holds no reachable node.
SegmentReleaseSafety == \A a \in Reachable : SegOf(a) \notin releasedSegments

\* (4) NoMarksWhileMutating — marks exist only during marking/sweeping.
NoMarksWhileMutating ==
    (phase = "mutating") => (\A a \in Addr : store[a] /= "marked")

\* (5) RootsAreOccupied — psi never names a free slot.
RootsAreOccupied == \A a \in psi : store[a] \in {"live","marked"}

\* (6) FreeListWellFormed — the free list names only free, non-released slots.
FreeListWellFormed ==
    /\ \A a \in freeList : store[a] = "free"
    /\ \A a \in freeList : SegOf(a) \notin releasedSegments

(*===========================================================================*)
(* GENERATIONAL ACTION PROPERTIES (the C1.b headline — checked as <>[]_vars). *)
(*===========================================================================*)

\* (G1) MinorSweepOnlyReclaimsYoung — every slot a MINOR transitions from
\* occupied to free is YOUNG (SegOf >= the PRE-state youngFloor). A minor never
\* reclaims an old slot. (The major has no such restriction.) This is the
\* defining generational guarantee.
MinorSweepOnlyReclaimsYoung ==
    [][ MinorSweep =>
          (\A a \in Addr :
              (store[a] \in {"live","marked"} /\ store'[a] = "free") => SegOf(a) >= youngFloor)
      ]_vars

\* (G2) MinorNeverFreesReachable — the action-level statement of (B): a minor
\* never frees a node reachable in the PRE-state. (Subsumed by NoUseAfterFree as
\* a state invariant, stated here as an explicit step guarantee.)
MinorNeverFreesReachable ==
    [][ MinorSweep =>
          (\A a \in Reachable : store'[a] /= "free")
      ]_vars

(*===========================================================================*)
(* C1.c COUNTEREXAMPLE-BY-CONSTRUCTION (documentation, not checked):          *)
(* A young-only mark would replace MarkStep's transitive rule with one that    *)
(* skips descending into OLD nodes (only pushes children of MARKED YOUNG       *)
(* nodes). With an OLD->YOUNG edge present (an old parent reachable from a      *)
(* root, pointing at a young child), that mark would leave the young child      *)
(* "live" (unmarked) though it is reachable; MinorSweep would then free it —    *)
(* violating NoUseAfterFree. This is exactly the free-list-reuse hazard in      *)
(* docs/cesk-gc/phase-c-generational-design.md (C1.c prerequisite). C1.b's      *)
(* FULL mark avoids it; C1.c must first make new allocation never land in old.  *)
(*===========================================================================*)

=============================================================================
