(** Counter-sync / GC free-phase exclusion.

    Periodic exec-counter sync reads live value slots and may materialize a
    MettaValue for tiered-cache accounting.  GC response processing and session
    release free value slots.  The live Rust implementation serializes these
    paths with COUNTER_FLUSH_LOCK; this proof captures the only property that
    matters for UAF prevention: a sync scanner and GC freer cannot both own the
    critical section.
 *)

Inductive Actor : Type :=
| CounterSync : Actor
| Collector : Actor.

Inductive LockOwner : Type :=
| NoOwner : LockOwner
| OwnedBy : Actor -> LockOwner.

Record State : Type := {
  owner : LockOwner;
  sync_scanning : bool;
  gc_freeing : bool
}.

Definition lock_well_formed (s : State) : Prop :=
  (sync_scanning s = true -> owner s = OwnedBy CounterSync) /\
  (gc_freeing s = true -> owner s = OwnedBy Collector).

Definition no_counter_sync_free_overlap (s : State) : Prop :=
  ~(sync_scanning s = true /\ gc_freeing s = true).

Theorem counter_flush_lock_excludes_gc_free_overlap :
  forall s,
    lock_well_formed s ->
    no_counter_sync_free_overlap s.
Proof.
  intros s [Hsync Hgc] [Hscan Hfree].
  specialize (Hsync Hscan).
  specialize (Hgc Hfree).
  rewrite Hsync in Hgc.
  discriminate Hgc.
Qed.

Theorem unlocked_overlap_counterexample :
  exists s,
    owner s = NoOwner /\
    sync_scanning s = true /\
    gc_freeing s = true /\
    ~ no_counter_sync_free_overlap s.
Proof.
  exists {| owner := NoOwner; sync_scanning := true; gc_freeing := true |}.
  split; [reflexivity |].
  split; [reflexivity |].
  split; [reflexivity |].
  unfold no_counter_sync_free_overlap.
  intro H.
  apply H.
  split; reflexivity.
Qed.
