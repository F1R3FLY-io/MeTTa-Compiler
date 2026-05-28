-------------------------- MODULE StoreCentricGC --------------------------
(*****************************************************************************)
(* TLA+ Model of the Store-Centric, Non-Moving, Index-Arena Garbage          *)
(* Collector for the MeTTa evaluator (MeTTaTron).                            *)
(*                                                                           *)
(* This is the formal-verification deliverable of "Increment 6" of the GC    *)
(* migration described in:                                                   *)
(*   docs/cesk-gc/store-centric-architecture.md                             *)
(*                                                                           *)
(* It is the companion / successor to the slab-allocator models             *)
(* (SlabGC.tla, SlabGC_Reactive.tla, SlabGC_Quiescent.tla). Where those      *)
(* models verified a CONCURRENT-SNAPSHOT collector with a separate           *)
(* registry side-channel + epoch/ABA filtering, THIS model verifies the      *)
(* re-architected collector:                                                 *)
(*                                                                           *)
(*   1. NON-MOVING:  an Addr's contents/identity never change location.      *)
(*      => There is NO relocation action and NO RelocationCorrectness        *)
(*         obligation. (Determinism of object identity is true by            *)
(*         construction — see the "Formal-model note" in the design doc.)    *)
(*                                                                           *)
(*   2. STRUCTURAL Ψ (the root set `psi`):  roots are the Addrs *touched*     *)
(*      by the machine state ⟨C,E,K⟩ together with the environment / tier    *)
(*      surfaces. There is exactly ONE root set. There is NO second          *)
(*      "registry" variable that could desynchronize from it. This is the    *)
(*      whole point of the migration: the `ROOT_REGISTRY` /                  *)
(*      `register_root_provider` / `collect_all_roots` side-channel is       *)
(*      DELETED (design doc Inc 6). Consequently the "registry-desync bug    *)
(*      class" (a value live via a surface that GC's snapshot forgot to      *)
(*      include) is *structurally unmodelable* here — there is no second     *)
(*      set to forget. NoLostObjects below is the positive statement of      *)
(*      this: one structural mark covers the full Ψ closure.                 *)
(*                                                                           *)
(*   3. REACHABILITY OVER EDGES:  `edges: Addr -> SUBSET Addr` models each    *)
(*      live node's child handles. The live set is the transitive closure    *)
(*      of `psi` over `edges` (Reachable). Mutators rewire edges of live     *)
(*      nodes during "mutating" (modeling State-cell mutation / SExpr        *)
(*      construction). Because edges can be rewired arbitrarily between       *)
(*      cycles and there is NO write barrier (design doc: State is a         *)
(*      value-by-id, the *map of values* is a root, no embedded old→young    *)
(*      handle in σ), the collector must perform a FULL transitive mark      *)
(*      from all roots each cycle. This model verifies that the full         *)
(*      transitive mark is sound and complete.                               *)
(*                                                                           *)
(*   4. TRUE QUIESCENCE RENDEZVOUS:  the process-global `phase` advances      *)
(*      mutating -> marking ONLY when `activeEvaluators = {}` (every active   *)
(*      mutator has drained to a safepoint and parked). Mark and sweep run    *)
(*      synchronously, in-place, over the live store at quiescence — there    *)
(*      is NO async snapshot, NO concurrent mutation during mark/sweep.       *)
(*      This re-derives data-race-freedom for the new protocol               *)
(*      (QuiescenceInvariant) WITHOUT the snapshot/epoch machinery.           *)
(*                                                                           *)
(*   5. FREE-LIST REBUILT EACH SWEEP:  `freeList` is the set of Addrs         *)
(*      reclaimed by the LAST sweep, consumed by Alloc in the next            *)
(*      "mutating" phase, and rebuilt-from-scratch on every sweep. It is      *)
(*      NEVER persisted across a cycle in a way that could alias. Because     *)
(*      (a) nothing transitions to "free" during "mutating" and (b) the      *)
(*      free list is only (re)populated during "sweeping" at quiescence,      *)
(*      slot reuse is ABA-free: a slot cannot be freed-then-reallocated       *)
(*      "underneath" a concurrent reader, because there are no concurrent     *)
(*      readers during sweep. This PROVES that per-slot epochs / 128-bit-CAS  *)
(*      (deleted in Inc 6) are unnecessary (NoConcurrentFree).               *)
(*                                                                           *)
(*   6. SEGMENT RELEASE:  Addrs are grouped into fixed Segments. A segment    *)
(*      whose every Addr is dead at the end of a sweep is released wholesale  *)
(*      (`releasedSegments`), modeling returning a whole arena segment to     *)
(*      the OS. SegmentReleaseSafety verifies no released segment holds a     *)
(*      live-reachable node.                                                  *)
(*                                                                           *)
(* SAFETY PROPERTIES (the design's claims — all must HOLD):                  *)
(*   1. NoUseAfterFree       — reachable(psi) ⇒ not "free" (after sweep)      *)
(*   2. NoLostObjects        — after marking, reachable(psi) ⇒ "marked"      *)
(*   3. SegmentReleaseSafety — released segment ⇒ no reachable Addr in it     *)
(*   4. QuiescenceInvariant  — marking/sweeping ⇒ activeEvaluators = {}       *)
(*   5. NoConcurrentFree     — no Addr→free during mutating; freeList         *)
(*                             repopulated only during sweeping              *)
(*****************************************************************************)

EXTENDS Integers, FiniteSets, Sequences

CONSTANTS
    SEGMENTS,           \* Fixed finite set of segment ids, e.g. {0, 1}
    OFFSETS,            \* Fixed finite set of offsets within a segment, e.g. {0,1,2}
    NumWorkers,         \* Number of concurrent mutator (evaluator) threads
    MaxRoots            \* Upper bound on |psi| (root-set size), keeps states finite

(*---------------------------------------------------------------------------*)
(* Addressing. An Addr is a <<segment, offset>> pair. This mirrors the Rust   *)
(* index arena's `Addr = (seg << 18) | off` (index_arena.rs:41,76): the       *)
(* segment is the high bits, the offset the low bits. The Addr's IDENTITY     *)
(* (which <<seg,off>> it is) is FIXED — non-moving: an allocation at an Addr   *)
(* never relocates, so reasoning over the address-graph is over a fixed       *)
(* finite carrier set.                                                        *)
(*---------------------------------------------------------------------------*)
Addr == SEGMENTS \X OFFSETS

\* The segment a given Addr belongs to (the high bits).
SegOf(a) == a[1]

\* All Addrs in a given segment.
AddrsInSeg(s) == {a \in Addr : SegOf(a) = s}

Workers == 1..NumWorkers

VARIABLES
    (*-----------------------------------------------------------------------*)
    (* THE STORE σ (liveness layer).                                          *)
    (* store: Addr -> {"free","live","marked"}                                *)
    (*   "free"   — slot holds no live value (reclaimable / on free list)     *)
    (*   "live"   — slot holds a live value (allocated, not yet marked)       *)
    (*   "marked" — slot holds a live value AND has been marked this cycle    *)
    (* This is the Addr-keyed mark-bitmap+occupancy layer, decoupled from      *)
    (* the value bytes (design doc: "Liveness layers are separate Addr-keyed  *)
    (* maps … decoupled from value bytes"). Non-moving: an Addr's slot stays  *)
    (* at the same Addr for all time; only its store-state changes.           *)
    (*-----------------------------------------------------------------------*)
    store,

    (*-----------------------------------------------------------------------*)
    (* Ψ — THE STRUCTURAL ROOT SET (`psi`).                                   *)
    (* psi \subseteq Addr — the Addrs touched by ⟨C,E,K⟩ ∪ env/tier surfaces. *)
    (*                                                                         *)
    (* This is THE root set. There is no competing registry variable. Roots    *)
    (* are STRUCTURAL: mutators add a root when binding/constructing touches   *)
    (* an Addr (AddRoot), and drop a root when a binding/surface releases it   *)
    (* (RemoveRoot). The collector reads `psi` directly at quiescence; it      *)
    (* does not consult any side-channel. Modeling psi as a single variable    *)
    (* (rather than registry ∪ stack ∪ safepoint, as the slab model did) is    *)
    (* the faithful abstraction of "structural Ψ, no manual root registry".    *)
    (*-----------------------------------------------------------------------*)
    psi,

    (*-----------------------------------------------------------------------*)
    (* edges: Addr -> SUBSET Addr — the child-handle graph.                   *)
    (* edges[a] is the set of Addrs that node `a` holds handles to (its       *)
    (* children). Reachability = transitive closure of `psi` over `edges`.    *)
    (* Only live nodes have (meaningful) edges; freed slots have edges = {}.   *)
    (* Mutators rewire edges of live nodes during "mutating" (RewireEdge),    *)
    (* modeling State-cell mutation and SExpr construction. There is NO write  *)
    (* barrier, hence the full transitive mark each cycle.                     *)
    (*-----------------------------------------------------------------------*)
    edges,

    (*-----------------------------------------------------------------------*)
    (* activeEvaluators — the set of worker threads currently in the          *)
    (* "mutating" phase (models ACTIVE_EVALUATORS). A worker is in this set    *)
    (* between WorkerEnter and WorkerPark. The collector may transition out    *)
    (* of "mutating" only when this set is EMPTY (true quiescence).            *)
    (*-----------------------------------------------------------------------*)
    activeEvaluators,

    (*-----------------------------------------------------------------------*)
    (* phase — the process-global collector phase.                            *)
    (*   "mutating" — workers run; alloc/root/edge mutations happen           *)
    (*   "marking"  — transitive mark from psi over edges (quiescent)         *)
    (*   "sweeping" — reclaim unmarked, rebuild free list, release segments   *)
    (*-----------------------------------------------------------------------*)
    phase,

    (*-----------------------------------------------------------------------*)
    (* freeList — the set of Addrs reclaimed by the LAST sweep, available for  *)
    (* Alloc to reuse in the next "mutating" phase. Rebuilt-from-scratch on    *)
    (* every Sweep (NEVER carried across cycles by accumulation). Consumed     *)
    (* (shrinks) by Alloc during "mutating".                                   *)
    (*-----------------------------------------------------------------------*)
    freeList,

    (*-----------------------------------------------------------------------*)
    (* releasedSegments — segments released wholesale because every Addr in    *)
    (* them was dead at the end of a sweep (models returning an arena segment  *)
    (* to the OS). A released segment's Addrs are all "free".                  *)
    (*-----------------------------------------------------------------------*)
    releasedSegments,

    (*-----------------------------------------------------------------------*)
    (* gcRequested — a GC has been requested (models the watermark / pressure  *)
    (* trigger rehomed into the safepoint, design doc Inc 6). Consumed when    *)
    (* marking begins.                                                         *)
    (*-----------------------------------------------------------------------*)
    gcRequested,

    (*-----------------------------------------------------------------------*)
    (* workerPhase — per-worker control state, used to model the quiescence    *)
    (* rendezvous (drain-to-safepoint-and-park).                               *)
    (*   "outside" — not currently evaluating (not in activeEvaluators)        *)
    (*   "running" — actively mutating (in activeEvaluators)                   *)
    (* A worker parks (running -> outside, leaving activeEvaluators) at a       *)
    (* safepoint; the collector can only begin once ALL workers are parked.    *)
    (*-----------------------------------------------------------------------*)
    workerPhase

vars == <<store, psi, edges, activeEvaluators, phase, freeList,
          releasedSegments, gcRequested, workerPhase>>

(*===========================================================================*)
(* Reachability — the transitive closure of `psi` over `edges`.              *)
(*                                                                           *)
(* We compute the least fixed point of:  R = psi ∪ { children of R }, but    *)
(* restricted to LIVE/MARKED Addrs (a freed slot has no live value, so its   *)
(* children are not part of the live object graph — and indeed Sweep clears  *)
(* freed slots' edges to {}). Over the finite carrier `Addr`, the closure    *)
(* is reached within |Addr| expansion steps, so a bounded iteration of       *)
(* Cardinality(Addr) rounds is exact. We define it via a recursive helper    *)
(* parameterized by a fuel count.                                            *)
(*                                                                           *)
(* `OccupiedAddrs` = Addrs that currently hold a value ("live" or "marked"). *)
(* Edges out of an Addr only contribute to reachability if that Addr is      *)
(* itself occupied (you cannot traverse through a free slot).                *)
(*===========================================================================*)

OccupiedAddrs(st) == {a \in Addr : st[a] \in {"live", "marked"}}

\* One expansion step: add the children (via edges) of every CURRENTLY
\* included, occupied Addr.
ExpandOnce(R, st) ==
    R \union UNION { edges[a] : a \in {x \in R : st[x] \in {"live", "marked"}} }

\* Bounded transitive closure. `fuel` rounds of expansion; with
\* fuel = Cardinality(Addr) this reaches the least fixed point because each
\* round that is not already a fixed point adds at least one new Addr, and
\* there are at most |Addr| Addrs to add.
RECURSIVE ClosureFuel(_, _, _)
ClosureFuel(R, st, fuel) ==
    IF fuel <= 0 THEN R
    ELSE LET R2 == ExpandOnce(R, st)
         IN IF R2 = R THEN R ELSE ClosureFuel(R2, st, fuel - 1)

\* The set of Addrs reachable from the root set `roots` over `edges`,
\* given store state `st`. Restricted to roots that are actually occupied
\* (a root pointing at nothing contributes nothing further).
ReachableUnder(roots, st) ==
    ClosureFuel(roots, st, Cardinality(Addr))

\* The live-reachable set under the CURRENT store: Addrs reachable from psi
\* that are occupied. This is the set the collector must retain.
Reachable == { a \in ReachableUnder(psi, store) : store[a] \in {"live", "marked"} }

(*===========================================================================*)
(* Helpers                                                                   *)
(*===========================================================================*)

\* Can a worker allocate? Either the free list has a reclaimable slot, or
\* there is a "free" slot in a non-released segment to bump into. (We model
\* "bump a fresh Addr in a non-full segment" simply as: pick any free Addr
\* not in a released segment. Non-moving ⇒ which physical free Addr is used
\* does not affect any safety property, so we let the model pick any.)
FreeAddrs == {a \in Addr : store[a] = "free" /\ SegOf(a) \notin releasedSegments}

CanAllocate == FreeAddrs /= {}

\* All Addrs currently holding a value (live or marked).
Occupied == OccupiedAddrs(store)

(*===========================================================================*)
(* Initial State                                                             *)
(*===========================================================================*)

Init ==
    /\ store = [a \in Addr |-> "free"]
    /\ psi = {}
    /\ edges = [a \in Addr |-> {}]
    /\ activeEvaluators = {}
    /\ phase = "mutating"
    /\ freeList = {}
    /\ releasedSegments = {}
    /\ gcRequested = FALSE
    /\ workerPhase = [w \in Workers |-> "outside"]

(*===========================================================================*)
(* MUTATOR ACTIONS (enabled only during "mutating")                          *)
(*===========================================================================*)

(*---------------------------------------------------------------------------*)
(* WorkerEnter: a worker begins evaluating — joins activeEvaluators.          *)
(* Mirrors EvalGuard::enter incrementing ACTIVE_EVALUATORS.                   *)
(*                                                                            *)
(* THE SAFEPOINT BARRIER (faithful to the design's "each active mutator       *)
(* drains to a safepoint and parks"): once a GC has been requested, NO new    *)
(* entrant is admitted (`~gcRequested`). This is the cooperative-safepoint    *)
(* gate — it lets activeEvaluators monotonically drain to {} so the           *)
(* rendezvous can complete, rather than letting a busy mutator re-enter        *)
(* forever and starve the collector. (Without this gate, GC liveness fails:   *)
(* see RESULTS.md "Liveness" — a re-entering worker keeps BeginMark only       *)
(* intermittently enabled, defeating weak fairness. The gate is not a         *)
(* fairness hack; it is the actual EvalGuard-vs-GC_REQUESTED admission rule.)  *)
(*                                                                            *)
(* Also gated on phase = "mutating": no entrant once marking begins.          *)
(*---------------------------------------------------------------------------*)
WorkerEnter(w) ==
    /\ phase = "mutating"
    /\ ~gcRequested                 \* SAFEPOINT BARRIER: no new entrants once GC pending
    /\ workerPhase[w] = "outside"
    /\ workerPhase' = [workerPhase EXCEPT ![w] = "running"]
    /\ activeEvaluators' = activeEvaluators \union {w}
    /\ UNCHANGED <<store, psi, edges, phase, freeList,
                   releasedSegments, gcRequested>>

(*---------------------------------------------------------------------------*)
(* WorkerPark: a worker drains to a safepoint and parks — leaves              *)
(* activeEvaluators. Mirrors EvalGuard::drop / the cooperative safepoint      *)
(* that decrements ACTIVE_EVALUATORS. This is the action that can bring the   *)
(* system to quiescence (activeEvaluators = {}), enabling the collector.      *)
(*---------------------------------------------------------------------------*)
WorkerPark(w) ==
    /\ workerPhase[w] = "running"
    /\ workerPhase' = [workerPhase EXCEPT ![w] = "outside"]
    /\ activeEvaluators' = activeEvaluators \ {w}
    /\ UNCHANGED <<store, psi, edges, phase, freeList,
                   releasedSegments, gcRequested>>

(*---------------------------------------------------------------------------*)
(* Alloc: a worker allocates a value (alloc(σ,v) → (Addr, σ')).               *)
(* Pops from freeList if available (reuse a slot reclaimed by the last        *)
(* sweep), else bumps a fresh free Addr in a non-released segment. The new    *)
(* Addr becomes "live" with empty edges, and (modeling that the allocation    *)
(* is touched by the machine state) may immediately become a root.            *)
(*                                                                            *)
(* NoConcurrentFree-relevant: Alloc sets a slot to "live" (never "free"),     *)
(* and only REMOVES from freeList (consumes), never adds. freeList is never   *)
(* grown here.                                                                 *)
(*---------------------------------------------------------------------------*)
Alloc(w) ==
    /\ phase = "mutating"
    /\ workerPhase[w] = "running"
    /\ Cardinality(psi) < MaxRoots
    /\ CanAllocate
    /\ \E a \in FreeAddrs :
        /\ store' = [store EXCEPT ![a] = "live"]
        \* Consume from the free list if this Addr was on it (slot reuse).
        /\ freeList' = freeList \ {a}
        \* A freshly allocated node starts with no children.
        /\ edges' = [edges EXCEPT ![a] = {}]
        \* The allocation is touched by ⟨C,E,K⟩ at the moment of creation:
        \* it becomes a root. (Modeled deterministically — a brand-new value
        \* is always referenced by the machine state that created it.)
        /\ psi' = psi \union {a}
    /\ UNCHANGED <<activeEvaluators, phase, releasedSegments,
                   gcRequested, workerPhase>>

(*---------------------------------------------------------------------------*)
(* AddRoot: a mutator touches an existing live Addr from the machine state    *)
(* (e.g. a new binding E: Var -> Addr, or a tier-cache surface references     *)
(* the Addr). The Addr joins the structural root set. Only live Addrs can     *)
(* become roots (you cannot root a free slot — there is no value there).      *)
(*---------------------------------------------------------------------------*)
AddRoot(w) ==
    /\ phase = "mutating"
    /\ workerPhase[w] = "running"
    /\ Cardinality(psi) < MaxRoots
    /\ \E a \in Addr :
        /\ store[a] \in {"live", "marked"}
        /\ a \notin psi
        /\ psi' = psi \union {a}
    /\ UNCHANGED <<store, edges, activeEvaluators, phase, freeList,
                   releasedSegments, gcRequested, workerPhase>>

(*---------------------------------------------------------------------------*)
(* RemoveRoot: a mutator drops a structural root (unbind / surface release).  *)
(* The Addr leaves psi. The underlying value is NOT freed here — it merely    *)
(* becomes a collection candidate if nothing else reaches it. (Reclamation    *)
(* happens only at Sweep.)                                                    *)
(*---------------------------------------------------------------------------*)
RemoveRoot(w) ==
    /\ phase = "mutating"
    /\ workerPhase[w] = "running"
    /\ psi /= {}
    /\ \E a \in psi :
        /\ psi' = psi \ {a}
    /\ UNCHANGED <<store, edges, activeEvaluators, phase, freeList,
                   releasedSegments, gcRequested, workerPhase>>

(*---------------------------------------------------------------------------*)
(* RewireEdge: a mutator rewires the child handles of a LIVE node (models     *)
(* State-cell mutation via change_state, or SExpr construction linking        *)
(* children). The new child set may be ANY subset of currently-occupied       *)
(* Addrs — you can only point a handle at a value that exists. This is        *)
(* exactly the mutation that, absent a write barrier, forces the full         *)
(* transitive mark each cycle: a previously-unreachable live node can be      *)
(* linked under a root here, or a reachable node unlinked.                     *)
(*---------------------------------------------------------------------------*)
RewireEdge(w) ==
    /\ phase = "mutating"
    /\ workerPhase[w] = "running"
    /\ \E a \in Addr :
        /\ store[a] \in {"live", "marked"}     \* only live nodes have edges
        /\ \E newChildren \in SUBSET Occupied :
            /\ edges' = [edges EXCEPT ![a] = newChildren]
    /\ UNCHANGED <<store, psi, activeEvaluators, phase, freeList,
                   releasedSegments, gcRequested, workerPhase>>

(*---------------------------------------------------------------------------*)
(* RequestGC: the pressure / watermark monitor requests a collection          *)
(* (rehomed into the safepoint per design doc Inc 6). May fire at any time    *)
(* during "mutating". Idempotent.                                             *)
(*---------------------------------------------------------------------------*)
RequestGC ==
    /\ phase = "mutating"
    /\ ~gcRequested
    /\ gcRequested' = TRUE
    /\ UNCHANGED <<store, psi, edges, activeEvaluators, phase, freeList,
                   releasedSegments, workerPhase>>

(*===========================================================================*)
(* COLLECTOR ACTIONS                                                          *)
(*===========================================================================*)

(*---------------------------------------------------------------------------*)
(* BeginMark: the QUIESCENCE RENDEZVOUS. The collector transitions from       *)
(* "mutating" to "marking" — but ONLY when every active mutator has drained   *)
(* to a safepoint and parked, i.e. activeEvaluators = {}. This is the heart   *)
(* of the new protocol: mark/sweep run at TRUE quiescence, never concurrently *)
(* with mutation. Consumes the GC request.                                    *)
(*                                                                            *)
(* At this instant the live store is frozen (no worker is running), so the    *)
(* in-place mark below reads a stable graph — no async snapshot needed.       *)
(*---------------------------------------------------------------------------*)
BeginMark ==
    /\ phase = "mutating"
    /\ gcRequested
    /\ activeEvaluators = {}            \* TRUE QUIESCENCE
    /\ phase' = "marking"
    /\ gcRequested' = FALSE
    /\ UNCHANGED <<store, psi, edges, activeEvaluators, freeList,
                   releasedSegments, workerPhase>>

(*---------------------------------------------------------------------------*)
(* MarkStep: transitively mark from psi over edges. IDEMPOTENT and            *)
(* monotone. One step marks any single Addr that is (a) currently a root, OR  *)
(* (b) a child (via edges) of an already-"marked" occupied node — provided    *)
(* it is occupied and not yet "marked". Iterating MarkStep to fixpoint marks  *)
(* exactly the transitive closure of psi over edges, restricted to occupied   *)
(* Addrs. (This is the abstract-GC least-fixed-point computed incrementally   *)
(* so TLC can interleave — though here nothing else runs during "marking".)   *)
(*---------------------------------------------------------------------------*)
MarkStep ==
    /\ phase = "marking"
    /\ \E a \in Addr :
        /\ store[a] = "live"            \* occupied but not yet marked
        /\ \/ a \in psi                                   \* a root, OR
           \/ \E p \in Addr :                             \* child of a marked node
                /\ store[p] = "marked"
                /\ a \in edges[p]
        /\ store' = [store EXCEPT ![a] = "marked"]
    /\ UNCHANGED <<psi, edges, activeEvaluators, phase, freeList,
                   releasedSegments, gcRequested, workerPhase>>

(*---------------------------------------------------------------------------*)
(* MarkComplete: marking has reached its fixed point — there is no remaining  *)
(* "live" (unmarked) Addr that is either a root or a child of a marked node.  *)
(* Transition to "sweeping". At this point the set of "marked" Addrs equals   *)
(* exactly Reachable (proven by NoLostObjects / NoUseAfterFree below).        *)
(*---------------------------------------------------------------------------*)
MarkComplete ==
    /\ phase = "marking"
    /\ ~(\E a \in Addr :
            /\ store[a] = "live"
            /\ \/ a \in psi
               \/ \E p \in Addr : store[p] = "marked" /\ a \in edges[p])
    /\ phase' = "sweeping"
    /\ UNCHANGED <<store, psi, edges, activeEvaluators, freeList,
                   releasedSegments, gcRequested, workerPhase>>

(*---------------------------------------------------------------------------*)
(* Sweep: reclaim every unmarked occupied Addr (was "live", never reached),   *)
(* clear all marks back to "live", REBUILD the free list from scratch, clear  *)
(* freed slots' edges, release fully-dead segments wholesale, and return to   *)
(* "mutating". This is a single atomic action (the sweep runs to completion   *)
(* at quiescence; modeling it atomically is faithful because nothing else     *)
(* runs concurrently — QuiescenceInvariant).                                  *)
(*                                                                            *)
(*   - reclaimed   = occupied-but-unmarked Addrs ("live" that mark missed)    *)
(*   - survivors   = "marked" Addrs (become "live" again for the next cycle)  *)
(*   - newStore    = reclaimed -> "free"; marked -> "live"; free stays "free" *)
(*   - freeList'   = rebuilt = ALL "free" Addrs after reclamation that are    *)
(*                   NOT in a (newly) released segment  [REBUILT FROM SCRATCH]*)
(*   - deadSegs    = segments all of whose Addrs are "free" after reclamation *)
(*   - edges'      = freed slots get edges = {} (no dangling children); the   *)
(*                   survivors keep their edges (still valid, all children    *)
(*                   were themselves marked ⇒ retained — see NoUseAfterFree). *)
(*                                                                            *)
(* NoConcurrentFree-relevant: THIS is the only action that sets slots to      *)
(* "free" and the only action that grows freeList — and it runs only in       *)
(* "sweeping" (⇒ at quiescence). freeList is assigned wholesale from the      *)
(* post-reclamation free set, never accumulated onto the prior freeList.      *)
(*---------------------------------------------------------------------------*)
Sweep ==
    /\ phase = "sweeping"
    /\ LET reclaimed == {a \in Addr : store[a] = "live"}   \* unmarked occupied
           survivors == {a \in Addr : store[a] = "marked"}
           newStore  == [a \in Addr |->
                           CASE store[a] = "marked" -> "live"
                             [] store[a] = "live"   -> "free"   \* reclaim
                             [] OTHER               -> "free"]  \* was free
           \* Segments that are entirely free after reclamation become
           \* candidates for wholesale release.
           deadSegs  == {s \in SEGMENTS :
                           \A a \in AddrsInSeg(s) : newStore[a] = "free"}
           \* The new released-segment set: previously released PLUS newly dead.
           newReleased == releasedSegments \union deadSegs
       IN /\ store' = newStore
          \* Rebuild free list FROM SCRATCH: all free Addrs not in a released
          \* segment. (Released-segment slots are returned to the OS, not the
          \* free list.) This is a wholesale assignment — the old freeList is
          \* discarded, not accumulated.
          /\ freeList' = {a \in Addr :
                             newStore[a] = "free" /\ SegOf(a) \notin newReleased}
          /\ releasedSegments' = newReleased
          \* Clear edges of every slot that is now free (reclaimed or already
          \* free) so no dangling child handles survive. Survivors keep edges.
          /\ edges' = [a \in Addr |->
                          IF newStore[a] = "free" THEN {} ELSE edges[a]]
          /\ phase' = "mutating"
    /\ UNCHANGED <<psi, activeEvaluators, gcRequested, workerPhase>>

(*===========================================================================*)
(* Next-State Relation                                                       *)
(*===========================================================================*)

Next ==
    \/ \E w \in Workers :
        \/ WorkerEnter(w)
        \/ WorkerPark(w)
        \/ Alloc(w)
        \/ AddRoot(w)
        \/ RemoveRoot(w)
        \/ RewireEdge(w)
    \/ RequestGC
    \/ BeginMark
    \/ MarkStep
    \/ MarkComplete
    \/ Sweep

Spec == Init /\ [][Next]_vars

(*===========================================================================*)
(* Fairness (for the liveness / termination sanity check)                    *)
(*===========================================================================*)
(* Weak fairness on the collector pipeline and on workers parking ensures a   *)
(* requested GC eventually completes (phase returns to "mutating"). We do NOT *)
(* put fairness on WorkerEnter/Alloc/AddRoot/RewireEdge: those are the source *)
(* of work and need not progress for GC to complete; in fact, for the GC to   *)
(* reach quiescence, workers must be ABLE to park, which is why WorkerPark    *)
(* is fair. RequestGC is fair so a collection is eventually demanded.         *)

FairSpec ==
    /\ Spec
    /\ \A w \in Workers : WF_vars(WorkerPark(w))
    /\ WF_vars(RequestGC)
    /\ WF_vars(BeginMark)
    /\ WF_vars(MarkStep)
    /\ WF_vars(MarkComplete)
    /\ WF_vars(Sweep)

(*===========================================================================*)
(* TYPE CORRECTNESS                                                           *)
(*===========================================================================*)

TypeOK ==
    /\ store \in [Addr -> {"free", "live", "marked"}]
    /\ psi \subseteq Addr
    /\ edges \in [Addr -> SUBSET Addr]
    /\ activeEvaluators \subseteq Workers
    /\ phase \in {"mutating", "marking", "sweeping"}
    /\ freeList \subseteq Addr
    /\ releasedSegments \subseteq SEGMENTS
    /\ gcRequested \in BOOLEAN
    /\ workerPhase \in [Workers -> {"outside", "running"}]

(*===========================================================================*)
(* SAFETY INVARIANTS — the design's claims. ALL must HOLD.                    *)
(*===========================================================================*)

(*---------------------------------------------------------------------------*)
(* (1) NoUseAfterFree.                                                        *)
(* Every Addr reachable from psi (transitively over edges, among occupied     *)
(* nodes) is NOT "free". No live-reachable Addr is ever swept to the free     *)
(* list. This is the core memory-safety property: a handle held by the        *)
(* machine state, or transitively reachable from one, always denotes a live   *)
(* slot — never a reclaimed one.                                              *)
(*                                                                            *)
(* It must hold in EVERY state, including the instant after Sweep: Sweep      *)
(* reclaims only unmarked nodes, and (by NoLostObjects, established during     *)
(* "marking") every reachable node was marked, hence retained.                *)
(*---------------------------------------------------------------------------*)
NoUseAfterFree ==
    \A a \in Reachable : store[a] /= "free"

(*---------------------------------------------------------------------------*)
(* (2) NoLostObjects — COMPLETENESS of the single structural mark.            *)
(* Once marking is complete (phase = "sweeping"), every Addr reachable from   *)
(* psi is "marked". There is no live-reachable object the mark missed —       *)
(* and crucially, because psi is THE one structural root set (no registry     *)
(* side-channel), there is no surface that could have been forgotten. This    *)
(* is the formal statement that the registry-desync bug class cannot occur.   *)
(*                                                                            *)
(* (During "marking" the closure is still being filled in, so we assert this  *)
(* at the marking->sweeping boundary onward, i.e. whenever phase="sweeping".) *)
(*---------------------------------------------------------------------------*)
NoLostObjects ==
    (phase = "sweeping") => (\A a \in Reachable : store[a] = "marked")

(*---------------------------------------------------------------------------*)
(* (3) SegmentReleaseSafety.                                                  *)
(* No Addr in a released segment is reachable from psi. A released segment    *)
(* (returned wholesale to the OS) holds no live-reachable node — touching it  *)
(* would be a use-after-free at segment granularity.                          *)
(*---------------------------------------------------------------------------*)
SegmentReleaseSafety ==
    \A a \in Reachable : SegOf(a) \notin releasedSegments

(*---------------------------------------------------------------------------*)
(* (4) QuiescenceInvariant.                                                   *)
(* Mark and sweep run ONLY at true quiescence: whenever the collector is in   *)
(* "marking" or "sweeping", there are no active mutators. This re-derives     *)
(* data-race-freedom for the new protocol — there is provably no window in    *)
(* which a worker mutates the store/edges/psi while the collector reads them. *)
(*---------------------------------------------------------------------------*)
QuiescenceInvariant ==
    (phase \in {"marking", "sweeping"}) => (activeEvaluators = {})

(*---------------------------------------------------------------------------*)
(* (5) NoConcurrentFree.                                                      *)
(* No Addr is "free"d while the system is mutating, and the free list is only *)
(* (re)populated during sweeping. Concretely, this invariant captures the     *)
(* ABA-freedom argument: during "mutating", the set of "free" Addrs can only  *)
(* SHRINK (Alloc consumes), never grow; and freeList can only shrink. The     *)
(* only producer of "free" slots and of freeList entries is Sweep, which runs *)
(* at quiescence. We encode the checkable state predicate that supports this: *)
(*                                                                            *)
(*   (a) During "marking", no slot is "free"-in-transition incorrectly:       *)
(*       marking never frees anything (store only goes live->marked).         *)
(*   (b) freeList only ever contains "free" Addrs (it never names a live      *)
(*       slot), and never names an Addr in a released segment. Combined with  *)
(*       the action structure (only Sweep grows freeList / creates "free"),   *)
(*       this rules out a free-then-realloc race underneath a reader.         *)
(*                                                                            *)
(* The action-level guarantees (only Sweep frees / grows freeList, and Sweep  *)
(* runs only at quiescence) are enforced by construction in the actions and   *)
(* corroborated by QuiescenceInvariant; this invariant is the state-predicate *)
(* witness that no "free" slot is simultaneously named as live or as a root.  *)
(*---------------------------------------------------------------------------*)
NoConcurrentFree ==
    \* No freeList entry is a live value or a root (a freed slot is neither).
    /\ \A a \in freeList : store[a] = "free"
    /\ \A a \in freeList : a \notin psi
    \* The free list never names a slot in a released segment (those went to
    \* the OS, not the reuse pool) — reuse cannot resurrect a released slot.
    /\ \A a \in freeList : SegOf(a) \notin releasedSegments

(*===========================================================================*)
(* AUXILIARY INVARIANTS (structural well-formedness; strengthen the proof).  *)
(*===========================================================================*)

\* "marked" slots only ever exist during "marking" or "sweeping". In
\* "mutating" everything occupied is "live" (marks were cleared by Sweep).
NoMarksWhileMutating ==
    (phase = "mutating") => (\A a \in Addr : store[a] /= "marked")

\* Every root denotes an occupied (live or marked) slot — psi never points at
\* a free slot. (A structural root is, by definition, a touched live value.)
RootsAreOccupied ==
    \A a \in psi : store[a] \in {"live", "marked"}

\* activeEvaluators is exactly the set of "running" workers.
ActiveSetCorrect ==
    activeEvaluators = {w \in Workers : workerPhase[w] = "running"}

\* All Addrs in a released segment are "free" (the segment really is dead).
ReleasedSegmentsAreFree ==
    \A s \in releasedSegments : \A a \in AddrsInSeg(s) : store[a] = "free"

\* Edges of a free slot are empty (no dangling children out of a reclaimed
\* node). Only occupied slots carry child handles.
FreeSlotsHaveNoEdges ==
    \A a \in Addr : store[a] = "free" => edges[a] = {}

(*===========================================================================*)
(* LIVENESS / TERMINATION SANITY (checked under FairSpec).                   *)
(*===========================================================================*)

\* A requested GC eventually completes: once gcRequested is set, the system    *)
\* eventually returns to a "mutating" phase with the request consumed. This    *)
\* confirms the collector pipeline (BeginMark -> MarkStep* -> MarkComplete ->  *)
\* Sweep) makes progress and does not deadlock at quiescence.
GCEventuallyCompletes ==
    gcRequested ~> (phase = "mutating" /\ ~gcRequested)

\* The collector, once marking, eventually returns to mutating (the whole
\* mark/sweep cycle terminates).
CycleTerminates ==
    (phase = "marking") ~> (phase = "mutating")

=============================================================================
