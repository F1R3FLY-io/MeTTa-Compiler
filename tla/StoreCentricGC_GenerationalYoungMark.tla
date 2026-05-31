----------------- MODULE StoreCentricGC_GenerationalYoungMark -----------------
(*****************************************************************************)
(* TLA+ model of the YOUNG-ONLY MARK soundness for Phase C1.c — the cheap    *)
(* generational minor. Companion to StoreCentricGC_Generational.tla (which   *)
(* verified the FULL-mark minor). This model verifies the part C1.c adds:    *)
(* a minor that MARKS ONLY YOUNG nodes (skipping old) is sound IFF there is   *)
(* no old→young σ edge — and that the allocator's reuse policy is what        *)
(* establishes that invariant.                                               *)
(*                                                                           *)
(* THE QUESTION (the soundness linchpin):                                    *)
(*   A young-only mark descends only through YOUNG nodes from YOUNG roots,    *)
(*   skipping old nodes entirely. It is sound iff every truly-reachable YOUNG *)
(*   node is still marked (else `sweep_young` reclaims a live young node →    *)
(*   use-after-free). That holds iff NO old node points at a young node       *)
(*   (NoOldToYoungEdge): then a young node's parent is never old, so it is    *)
(*   reachable via an all-young path from a young root.                      *)
(*                                                                           *)
(* THE ALLOCATOR POLICY UNDER TEST (`ReuseCurSegOnly`):                       *)
(*   σ `Node` edges are IMMUTABLE post-publish (no RewireEdge here — a        *)
(*   `change-state!` young value is an E₀ root, not a σ edge), and bump       *)
(*   allocation places a node in `curSeg` (the highest open segment) so its   *)
(*   children (allocated no later ⇒ segment ≤ curSeg) satisfy BUMP ORDER      *)
(*   (child.seg ≤ node.seg). Promotion is `youngFloor := curSeg`. Bump order  *)
(*   ⇒ a node and all its children share segments ≤ its own ⇒ they PROMOTE    *)
(*   TOGETHER ⇒ no old→young edge.                                           *)
(*                                                                           *)
(*   FREE-SLOT REUSE can BREAK bump order: a node reused into a LOWER young   *)
(*   segment R < curSeg can point at a child in a HIGHER young segment        *)
(*   (≤ curSeg) — child.seg > R = node.seg. After the next promotion          *)
(*   (youngFloor := curSeg) that node (seg R < curSeg) is OLD while its child *)
(*   (seg = curSeg) is YOUNG ⇒ an old→young edge ⇒ the young-only mark misses *)
(*   the live young child ⇒ UAF.                                             *)
(*                                                                           *)
(*   `ReuseCurSegOnly = TRUE` models the C1.c fix: reuse ONLY `curSeg` free   *)
(*   slots, so every (re)allocated node is in curSeg and its children are     *)
(*   ≤ curSeg = node.seg (bump order preserved) ⇒ NoOldToYoungEdge holds ⇒    *)
(*   YoungOnlyMarkReachesLiveYoung holds. (Positive model.)                   *)
(*   `ReuseCurSegOnly = FALSE` models the UNSOUND any-young reuse: TLC MUST    *)
(*   find a counterexample to YoungOnlyMarkReachesLiveYoung — proving the     *)
(*   cur_seg-only restriction is LOAD-BEARING. (Negative model.)             *)
(*****************************************************************************)

EXTENDS Integers, FiniteSets

CONSTANTS
    SEGMENTS,          \* segment ids 0..N-1, e.g. {0,1,2}
    OFFSETS,           \* slots per segment, e.g. {0,1}
    MaxRoots,          \* bound on |psi|
    ReuseCurSegOnly    \* TRUE = C1.c fix (positive) ; FALSE = any-young reuse (negative)

Addr == SEGMENTS \X OFFSETS
SegOf(a) == a[1]
MaxSeg == CHOOSE s \in SEGMENTS : \A t \in SEGMENTS : t <= s

VARIABLES
    store,         \* Addr -> {"free","live","marked"}
    edges,         \* Addr -> SUBSET Addr  (set at Alloc; immutable thereafter)
    psi,           \* SUBSET Addr — the structural roots
    curSeg,        \* current bump segment (highest open)
    youngFloor,    \* segments >= youngFloor are YOUNG
    phase          \* {"mutating","marking","sweeping"}

vars == <<store, edges, psi, curSeg, youngFloor, phase>>

IsYoung(a) == SegOf(a) >= youngFloor
IsOld(a)   == SegOf(a) <  youngFloor
Occupied   == {a \in Addr : store[a] \in {"live","marked"}}

(* True (full) reachability of psi over edges among occupied nodes — what the
   collector MUST retain. Bounded closure (same technique as the base spec). *)
ExpandOnce(R) == R \union UNION { edges[a] : a \in {x \in R : store[x] \in {"live","marked"}} }
RECURSIVE ClosureFuel(_, _)
ClosureFuel(R, fuel) == IF fuel <= 0 THEN R
                        ELSE LET R2 == ExpandOnce(R) IN IF R2 = R THEN R ELSE ClosureFuel(R2, fuel-1)
Reachable == { a \in ClosureFuel(psi, Cardinality(Addr)) : store[a] \in {"live","marked"} }

(* Free slots eligible for (re)allocation under the policy:
   - always: a fresh/reclaimed free slot in curSeg (bump or cur_seg reuse);
   - if ~ReuseCurSegOnly: ALSO any free slot in a young segment (the unsound
     any-young reuse the negative model exercises). *)
AllocTargets ==
    IF ReuseCurSegOnly
    THEN {a \in Addr : store[a] = "free" /\ SegOf(a) = curSeg}
    ELSE {a \in Addr : store[a] = "free" /\ SegOf(a) >= youngFloor}

Init ==
    /\ store = [a \in Addr |-> "free"]
    /\ edges = [a \in Addr |-> {}]
    /\ psi = {}
    /\ curSeg = 0
    /\ youngFloor = 0
    /\ phase = "mutating"

(* Alloc: place a node at a target slot; its children are any currently-occupied
   nodes (modeling SExpr construction over existing values — they were allocated
   no later, so their segments are <= curSeg). edges are set HERE and never
   mutated (σ immutability). The node may become a root. *)
Alloc ==
    /\ phase = "mutating"
    /\ Cardinality(psi) < MaxRoots
    /\ \E a \in AllocTargets :
        /\ \E kids \in SUBSET Occupied :
            /\ store' = [store EXCEPT ![a] = "live"]
            /\ edges' = [edges EXCEPT ![a] = kids]
            /\ psi' = psi \union {a}
    /\ UNCHANGED <<curSeg, youngFloor, phase>>

(* OpenSeg: fill curSeg and advance to the next segment (the bump target moves
   up). Modeled as curSeg++ when a higher segment exists. *)
OpenSeg ==
    /\ phase = "mutating"
    /\ curSeg < MaxSeg
    /\ curSeg' = curSeg + 1
    /\ UNCHANGED <<store, edges, psi, youngFloor, phase>>

RemoveRoot ==
    /\ phase = "mutating"
    /\ psi /= {}
    /\ \E a \in psi : psi' = psi \ {a}
    /\ UNCHANGED <<store, edges, curSeg, youngFloor, phase>>

(* ---- the collector: young-only mark, then minor sweep + promote ---- *)
BeginMark ==
    /\ phase = "mutating"
    /\ phase' = "marking"
    /\ UNCHANGED <<store, edges, psi, curSeg, youngFloor>>

(* YOUNG-ONLY mark step: blacken a YOUNG live node that is a root OR a child of
   an already-marked YOUNG node. Old nodes are NEVER marked and NEVER descended
   (mirrors mark_young_from_roots_with: push/mark only seg >= young_floor). *)
YoungMarkStep ==
    /\ phase = "marking"
    /\ \E a \in Addr :
        /\ store[a] = "live"
        /\ IsYoung(a)
        /\ \/ a \in psi
           \/ \E p \in Addr : store[p] = "marked" /\ IsYoung(p) /\ a \in edges[p]
        /\ store' = [store EXCEPT ![a] = "marked"]
    /\ UNCHANGED <<edges, psi, curSeg, youngFloor, phase>>

MarkComplete ==
    /\ phase = "marking"
    /\ ~(\E a \in Addr :
            /\ store[a] = "live" /\ IsYoung(a)
            /\ \/ a \in psi
               \/ \E p \in Addr : store[p] = "marked" /\ IsYoung(p) /\ a \in edges[p])
    /\ phase' = "sweeping"
    /\ UNCHANGED <<store, edges, psi, curSeg, youngFloor>>

(* MinorSweep: reclaim unmarked YOUNG live nodes (the dead young); clear marks;
   leave OLD nodes untouched; then PROMOTE (youngFloor := curSeg) and return to
   mutating. Reclaimed slots' edges cleared. *)
MinorSweep ==
    /\ phase = "sweeping"
    /\ LET newStore == [a \in Addr |->
                          CASE store[a] = "marked"             -> "live"   \* survivor (young) — clear mark
                            [] store[a] = "live" /\ IsYoung(a) -> "free"   \* YOUNG unmarked — reclaim
                            [] OTHER                           -> store[a]] \* old / free untouched
       IN /\ store' = newStore
          /\ edges' = [a \in Addr |-> IF newStore[a] = "free" THEN {} ELSE edges[a]]
          /\ youngFloor' = curSeg     \* promote
          /\ phase' = "mutating"
    /\ UNCHANGED <<psi, curSeg>>

Next ==
    \/ Alloc \/ OpenSeg \/ RemoveRoot
    \/ BeginMark \/ YoungMarkStep \/ MarkComplete \/ MinorSweep

Spec == Init /\ [][Next]_vars

(*===========================================================================*)
(* TYPE + SAFETY                                                             *)
(*===========================================================================*)
TypeOK ==
    /\ store \in [Addr -> {"free","live","marked"}]
    /\ edges \in [Addr -> SUBSET Addr]
    /\ psi \subseteq Addr
    /\ curSeg \in SEGMENTS
    /\ youngFloor \in 0..(MaxSeg + 1)
    /\ phase \in {"mutating","marking","sweeping"}

(* (1) NoOldToYoungEdge — the σ invariant the young-only mark relies on: no
   occupied OLD node points at an occupied YOUNG node. HOLDS under
   ReuseCurSegOnly (bump order); VIOLATED under any-young reuse. *)
NoOldToYoungEdge ==
    \A a \in Occupied : IsOld(a) =>
        \A c \in edges[a] : (c \in Occupied) => IsOld(c)

(* (2) YoungOnlyMarkReachesLiveYoung — THE soundness property: once the
   young-only mark is complete (phase="sweeping"), every TRULY-reachable YOUNG
   node is marked, so MinorSweep does not reclaim a live young node (no UAF).
   This is what FAILS in the negative model. *)
YoungOnlyMarkReachesLiveYoung ==
    (phase = "sweeping") => (\A a \in Reachable : IsYoung(a) => store[a] = "marked")

(* (3) NoUseAfterFree — after a minor sweep, no truly-reachable node is free.
   (Reachable old nodes are never swept by a minor; reachable young must be
   marked by (2).) *)
NoUseAfterFree == \A a \in Reachable : store[a] /= "free"

=============================================================================
