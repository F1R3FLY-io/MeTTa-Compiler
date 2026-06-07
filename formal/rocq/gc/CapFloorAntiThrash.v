(** B.5 cap-floor anti-thrash obligations.

    A cap-triggered major that releases no segment is futile for RSS: the heap
    is still over the base cap after the sweep.  The collector therefore raises
    [CAP_FLOOR] to the current committed bytes.  With no further committed
    growth, the cap predicate cannot immediately re-fire.  A major that releases
    any segment clears the floor, restoring the ordinary base-cap predicate.
*)

From Stdlib Require Import Arith.
From Stdlib Require Import Lia.

Module MeTTaTron_GC_CapFloorAntiThrash.

Section CapFloorAntiThrashModel.
  Definition CapDue (Committed BaseCap CapFloor : nat) : Prop :=
    Committed > Nat.max BaseCap CapFloor.

  Definition FutileCapMajor
      (Committed BaseCap CapFloor ReleasedSegments : nat) : Prop :=
    CapDue Committed BaseCap CapFloor /\ ReleasedSegments = 0.

  Theorem raised_floor_blocks_immediate_cap_refire :
    forall Committed BaseCap NextFloor : nat,
      NextFloor = Committed ->
      ~ CapDue Committed BaseCap NextFloor.
  Proof.
    intros Committed BaseCap NextFloor Hfloor Hcap_due.
    subst NextFloor.
    unfold CapDue in Hcap_due.
    assert (Committed <= Nat.max BaseCap Committed) by apply Nat.le_max_r.
    lia.
  Qed.

  Theorem cap_refire_after_raised_floor_requires_growth :
    forall CommittedAfterSweep CommittedNext BaseCap NextFloor : nat,
      NextFloor = CommittedAfterSweep ->
      CapDue CommittedNext BaseCap NextFloor ->
      CommittedNext > CommittedAfterSweep.
  Proof.
    intros CommittedAfterSweep CommittedNext BaseCap NextFloor Hfloor Hcap_due.
    subst NextFloor.
    unfold CapDue in Hcap_due.
    assert (CommittedAfterSweep <= Nat.max BaseCap CommittedAfterSweep)
      by apply Nat.le_max_r.
    lia.
  Qed.

  Theorem futile_cap_major_raise_prevents_same_committed_refire :
    forall Committed BaseCap OldFloor ReleasedSegments NextFloor : nat,
      FutileCapMajor Committed BaseCap OldFloor ReleasedSegments ->
      NextFloor = Committed ->
      ~ CapDue Committed BaseCap NextFloor.
  Proof.
    intros Committed BaseCap OldFloor ReleasedSegments NextFloor _ Hfloor.
    apply raised_floor_blocks_immediate_cap_refire.
    exact Hfloor.
  Qed.

  Theorem cleared_floor_restores_base_cap_trigger :
    forall Committed BaseCap NextFloor : nat,
      NextFloor = 0 ->
      Committed > BaseCap ->
      CapDue Committed BaseCap NextFloor.
  Proof.
    intros Committed BaseCap NextFloor Hfloor Hover_base.
    subst NextFloor.
    unfold CapDue.
    rewrite Nat.max_0_r.
    exact Hover_base.
  Qed.

  Theorem release_cleared_floor_has_no_stale_floor :
    forall ReleasedSegments NextFloor : nat,
      ReleasedSegments > 0 ->
      NextFloor = 0 ->
      NextFloor = 0.
  Proof.
    intros ReleasedSegments NextFloor _ Hclear.
    exact Hclear.
  Qed.
End CapFloorAntiThrashModel.

End MeTTaTron_GC_CapFloorAntiThrash.
