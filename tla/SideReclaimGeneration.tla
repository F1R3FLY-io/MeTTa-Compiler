---- MODULE SideReclaimGeneration ----
\* Per-cell GENERATION guard for side-index reuse — the ABA fix (commit f6dd7a76).
\*
\* The companion model SideReclaimSnapshot.tla abstracts a side identity as the
\* 2-element set {"old","new"}: a reuse there always mints a NEW symbol, so the
\* stale "old" snapshot can never collide with the live "new" occupant. That
\* abstraction therefore CANNOT express the actual ABA (a stale snapshot naming
\* the SAME index a live node has since reused). This module closes that seam.
\*
\* It models ONE side cell as a succession of OCCUPANTS, each interned at a fresh,
\* strictly-monotone generation (SideColumn::push: slot.0 = slot.0.wrapping_add(1),
\* modelled as +1 over Naturals). A SideReclaim snapshot captures the cell's
\* generation at reclaim time. SideColumn::free(idx, gen) drops the cell ONLY when
\* the snapshot's captured generation equals the cell's CURRENT generation
\* (GenerationGuard = TRUE). Without the guard (the pre-fix free-by-bare-index,
\* GenerationGuard = FALSE) a stale snapshot of a since-reused index frees the LIVE
\* reuser's payload -> a use-after-free ("live Spanned slot").
\*
\* Discriminator: NoLiveCellFreed holds IFF the generation guard is on. This is the
\* model-checked NON-VACUITY companion to the deductive Rocq proof
\* QuiescentSideIndexReuse.v::GenerationGuardSafety.

EXTENDS Naturals

CONSTANTS GenerationGuard,  \* TRUE = SideColumn::free checks the generation
          MaxGen            \* bound the monotone generation for a finite model

VARIABLES gen,    \* the cell's current generation (the live occupant's, if any)
          live,   \* is the cell's current occupant a LIVE (reachable) node?
          snaps,  \* the set of pending SideReclaim snapshot generations
          uaf     \* has a free dropped the cell while its occupant was LIVE?

Init ==
  /\ gen = 1
  /\ live = TRUE
  /\ snaps = {}
  /\ uaf = FALSE

\* The current occupant becomes unreachable (a later sweep will reclaim it).
Die ==
  /\ live = TRUE
  /\ live' = FALSE
  /\ UNCHANGED <<gen, snaps, uaf>>

\* A sweep snapshots the (now-dead) occupant's side index at the cell's current
\* generation (side_reclaim_for_addr captures sr.gen). Only dead occupants are
\* snapshotted (append_pending_side_reclaims runs on reclaimed slots).
Capture ==
  /\ live = FALSE
  /\ snaps' = snaps \cup {gen}
  /\ UNCHANGED <<gen, live, uaf>>

\* SideColumn::push re-claims the freed cell for a NEW live occupant, bumping the
\* per-cell generation. The old snapshots in `snaps` now name a strictly-smaller
\* (stale) generation than the live occupant.
Reuse ==
  /\ live = FALSE
  /\ gen < MaxGen
  /\ gen' = gen + 1
  /\ live' = TRUE
  /\ UNCHANGED <<snaps, uaf>>

\* Quiescent drain (free_pending_side_reclaims -> SideColumn::free). With the
\* guard, a snapshot frees the cell ONLY when its captured generation equals the
\* cell's current generation; without it, every snapshot frees by bare index.
\* Freeing while the current occupant is LIVE is the use-after-free.
FreedSnaps == IF GenerationGuard THEN {g \in snaps : g = gen} ELSE snaps

Drain ==
  /\ snaps # {}
  /\ uaf' = (uaf \/ (live /\ (FreedSnaps # {})))
  /\ snaps' = snaps \ FreedSnaps
  /\ UNCHANGED <<gen, live>>

Next == Die \/ Capture \/ Reuse \/ Drain

Spec == Init /\ [][Next]_<<gen, live, snaps, uaf>>

TypeOK ==
  /\ gen \in 1..MaxGen
  /\ live \in BOOLEAN
  /\ snaps \subseteq (1..MaxGen)
  /\ uaf \in BOOLEAN

\* SAFETY: the guarded free never drops a cell whose occupant is live.
NoLiveCellFreed == uaf = FALSE

====
