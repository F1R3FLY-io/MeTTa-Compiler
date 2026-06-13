(** Bounded rendezvous side-reclaim progress (#273).

    Node-slot reuse + side-index reuse bound the NODE arena; this closes the
    remaining allocator-progress obligation for the SIDE-PAYLOAD columns under the
    default dedicated (rendezvous) collector: after a collection the allocator makes
    BOUNDED progress on committed side storage.

    The contract (source-coupled to index_heap.rs):
    - A rendezvous/minor sweep that reclaims an owner slot does NOT free its side
      payload `Box` immediately; it APPENDS one `SideReclaim` snapshot per reclaimed
      owner (`append_pending_side_reclaims`) — the payload stays COMMITTED, deferred.
    - A true-quiescence MAJOR drains the ENTIRE pending vec in one pass
      (`free_pending_side_reclaims` via `std::mem::take`), freeing every deferred
      payload (`pending` -> 0, committed side storage -> the live high-water).
    - `pending_side_major` (`phase == "quiescence" && pending_side_reclaims > 0`)
      FORCES that major the moment a quiescence point is reached with pending > 0,
      so `pending` is emptied every quiescence and committed side storage cannot grow
      unboundedly across cycles.

    MAIN [side_committed_bounded]: under the committed invariant
    `committed = live + pending`, committed side storage is bounded by the live
    high-water plus the reclaims appended since the last quiescence drain;
    [forced_drain_reduces_committed_to_live] shows the forced major reduces committed
    storage back to exactly the live high-water. The trigger law is an explicit
    model contract premise on the trigger theorems, not a section-level
    hypothesis. NON-VACUITY
    [without_pending_trigger_side_grows_unbounded] exhibits the pre-266d19d
    rendezvous-only world, where — with no forced drain — `pending` accumulates past
    any bound. No admits/axioms. *)

From Stdlib Require Import Arith.Arith.
From Stdlib Require Import micromega.Lia.

Module MeTTaTron_GC_RendezvousSideReclaimProgress.

Section BoundedSideReclaimProgress.

  (* The committed-side accounting of one collector state: |pending_side_reclaims|,
     the committed side-payload Boxes (live OR dead-but-pending-drain), and the live
     side-payload high-water this interval. *)
  Record SideState := {
    pending : nat;
    committed_side : nat;
    live_side : nat
  }.

  (* Every committed side payload is either LIVE or DEAD-but-PENDING-drain. *)
  Definition CommittedInvariant (s : SideState) : Prop :=
    committed_side s = live_side s + pending s.

  (* A rendezvous/minor sweep reclaims [r] owner slots: `append_pending_side_reclaims`
     pushes one `SideReclaim` per reclaimed owner (so [pending] grows by [r]); the
     reclaimed owners stop being live; the payload Boxes are NOT freed yet (committed
     unchanged). *)
  Definition RendezvousAppend (before after : SideState) (r : nat) : Prop :=
    pending after = pending before + r /\
    committed_side after = committed_side before /\
    live_side after = live_side before - r.

  (* A true-quiescence MAJOR drain: `free_pending_side_reclaims` does `mem::take`, so
     EVERY pending snapshot is consumed ([pending] -> 0) and its dead payload Box freed
     (committed -> the live high-water). *)
  Definition QuiescenceDrain (before after : SideState) : Prop :=
    pending after = 0 /\
    committed_side after = live_side before /\
    live_side after = live_side before.

  (* THEOREM 1 — each rendezvous appends exactly one pending entry per reclaimed owner
     (couples `append_pending_side_reclaims`). *)
  Theorem rendezvous_appends_one_per_node_reclaim :
    forall before after r,
      RendezvousAppend before after r ->
      pending after = pending before + r.
  Proof. intros before after r [Hp _]. exact Hp. Qed.

  (* THEOREM 2 — a quiescence drain empties the pending queue (couples the exhaustive
     `std::mem::take` in `free_pending_side_reclaims`). *)
  Theorem quiescence_drain_empties_pending :
    forall before after,
      QuiescenceDrain before after ->
      pending after = 0.
  Proof. intros before after [Hp _]. exact Hp. Qed.

  (* THEOREM 2b — and a drain re-establishes the invariant with committed = live (no
     dead payload remains committed). *)
  Theorem quiescence_drain_committed_equals_live :
    forall before after,
      QuiescenceDrain before after ->
      CommittedInvariant after.
  Proof.
    intros before after [Hp [Hc Hl]]. unfold CommittedInvariant.
    rewrite Hc, Hl, Hp. lia.
  Qed.

  (* The committed invariant is PRESERVED by a rendezvous append (the dead owners just
     move from the live count to the pending count; committed is unchanged). *)
  Theorem rendezvous_append_preserves_invariant :
    forall before after r,
      r <= live_side before ->
      CommittedInvariant before ->
      RendezvousAppend before after r ->
      CommittedInvariant after.
  Proof.
    intros before after r Hle Hinv [Hp [Hc Hl]].
    unfold CommittedInvariant in *.
    rewrite Hc, Hp, Hl. lia.
  Qed.

  (* The forced-drain law (`pending_side_major`): a quiescence point reached with
     pending > 0 PERFORMS a quiescence drain to the next state.  It is modeled as
     an explicit contract for the trigger function rather than a global proof
     assumption, so every theorem that uses it names the obligation it consumes. *)
  Definition PendingSideTriggerForces
      (Quiescent : SideState -> Prop)
      (step_to : SideState -> SideState) : Prop :=
    forall s, Quiescent s -> pending s > 0 -> QuiescenceDrain s (step_to s).

  (* THEOREM 3 — the trigger forces the full quiescence drain relation. *)
  Theorem pending_side_trigger_forces_quiescence_drain :
    forall Quiescent step_to s,
      PendingSideTriggerForces Quiescent step_to ->
      Quiescent s -> pending s > 0 -> QuiescenceDrain s (step_to s).
  Proof.
    intros Quiescent step_to s Htrigger Hq Hpos.
    apply Htrigger; assumption.
  Qed.

  (* THEOREM 3b — consequently, the trigger empties pending. *)
  Theorem pending_side_trigger_forces_drain :
    forall Quiescent step_to s,
      PendingSideTriggerForces Quiescent step_to ->
      Quiescent s -> pending s > 0 -> pending (step_to s) = 0.
  Proof.
    intros Quiescent step_to s Htrigger Hq Hpos.
    apply (quiescence_drain_empties_pending s (step_to s)).
    apply (pending_side_trigger_forces_quiescence_drain
             Quiescent step_to s Htrigger Hq Hpos).
  Qed.

  (* MAIN BOUNDED PROGRESS — under the committed invariant, committed side storage is
     bounded by the live high-water plus the reclaims pending since the last drain. *)
  Theorem side_committed_bounded :
    forall s,
      CommittedInvariant s ->
      committed_side s <= live_side s + pending s.
  Proof.
    intros s Hinv. unfold CommittedInvariant in Hinv. lia.
  Qed.

  (* The bounded-progress PAYOFF — after the forced major at a quiescence point with
     pending > 0, committed side storage DECREASES to exactly the live high-water. *)
  Theorem forced_drain_reduces_committed_to_live :
    forall Quiescent step_to s,
      PendingSideTriggerForces Quiescent step_to ->
      Quiescent s -> pending s > 0 ->
      committed_side (step_to s) = live_side s.
  Proof.
    intros Quiescent step_to s Htrigger Hq Hpos.
    destruct (Htrigger s Hq Hpos) as [_ [Hc _]].
    exact Hc.
  Qed.

  (* The same forced drain re-establishes committed = live + pending for the next
     state; since pending is zero, this is the post-major compacted invariant. *)
  Theorem forced_drain_reestablishes_committed_invariant :
    forall Quiescent step_to s,
      PendingSideTriggerForces Quiescent step_to ->
      Quiescent s -> pending s > 0 ->
      CommittedInvariant (step_to s).
  Proof.
    intros Quiescent step_to s Htrigger Hq Hpos.
    apply (quiescence_drain_committed_equals_live s (step_to s)).
    apply Htrigger; assumption.
  Qed.

  (* NON-VACUITY — WITHOUT the forced-drain trigger (the pre-266d19d rendezvous-only
     world), pending accumulates without bound: [k] rendezvous appends of [r > 0] each,
     with NO intervening drain, leave pending = p0 + k*r, which exceeds any bound. *)
  Fixpoint rendezvous_only_pending (p0 r k : nat) : nat :=
    match k with
    | 0 => p0
    | S k' => rendezvous_only_pending p0 r k' + r
    end.

  Theorem without_pending_trigger_side_grows_unbounded :
    forall p0 r B,
      r > 0 ->
      exists k, rendezvous_only_pending p0 r k > B.
  Proof.
    intros p0 r B Hr. exists (S B).
    assert (Hmono : forall k, rendezvous_only_pending p0 r k >= p0 + k * r).
    { induction k as [| k' IH]; simpl; lia. }
    specialize (Hmono (S B)). nia.
  Qed.

End BoundedSideReclaimProgress.

End MeTTaTron_GC_RendezvousSideReclaimProgress.
