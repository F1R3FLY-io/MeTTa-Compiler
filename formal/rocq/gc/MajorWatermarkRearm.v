(** B.4 old-live major watermark rearm obligations.

    The major trigger and rearm metric are both post-promote old-live bytes.
    With the implementation's growth factor of 2, rearming the watermark to
    [max(old_live_after * 2, min_threshold)] prevents an immediate live-growth
    major re-fire and requires future old-live bytes to exceed that doubled
    rearm metric before the live-growth clause can fire again.
*)

From Stdlib Require Import Arith.
From Stdlib Require Import Lia.

Module MeTTaTron_GC_MajorWatermarkRearm.

Section MajorWatermarkRearmModel.
  Definition RearmedWatermark (OldLiveAfter MinThreshold : nat) : nat :=
    Nat.max (OldLiveAfter + OldLiveAfter) MinThreshold.

  Definition LiveMajorDue (OldLive Watermark MinThreshold : nat) : Prop :=
    OldLive > Nat.max Watermark MinThreshold.

  Theorem rearmed_watermark_blocks_immediate_live_major_refire :
    forall OldLiveAfter MinThreshold : nat,
      ~ LiveMajorDue OldLiveAfter
          (RearmedWatermark OldLiveAfter MinThreshold)
          MinThreshold.
  Proof.
    intros OldLiveAfter MinThreshold Hdue.
    unfold LiveMajorDue, RearmedWatermark in Hdue.
    assert (OldLiveAfter <= Nat.max (OldLiveAfter + OldLiveAfter) MinThreshold).
    {
      apply Nat.le_trans with (m := OldLiveAfter + OldLiveAfter).
      - lia.
      - apply Nat.le_max_l.
    }
    assert (Nat.max (OldLiveAfter + OldLiveAfter) MinThreshold <=
            Nat.max (Nat.max (OldLiveAfter + OldLiveAfter) MinThreshold)
                    MinThreshold) by apply Nat.le_max_l.
    lia.
  Qed.

  Theorem live_major_refire_requires_doubled_old_growth :
    forall OldLiveAfter OldLiveNext MinThreshold : nat,
      LiveMajorDue OldLiveNext
        (RearmedWatermark OldLiveAfter MinThreshold)
        MinThreshold ->
      OldLiveNext > OldLiveAfter + OldLiveAfter.
  Proof.
    intros OldLiveAfter OldLiveNext MinThreshold Hdue.
    unfold LiveMajorDue, RearmedWatermark in Hdue.
    assert (OldLiveAfter + OldLiveAfter <=
            Nat.max (Nat.max (OldLiveAfter + OldLiveAfter) MinThreshold)
                    MinThreshold).
    {
      apply Nat.le_trans with
        (m := Nat.max (OldLiveAfter + OldLiveAfter) MinThreshold).
      - apply Nat.le_max_l.
      - apply Nat.le_max_l.
    }
    lia.
  Qed.

  Theorem live_major_refire_requires_min_threshold_growth :
    forall OldLiveAfter OldLiveNext MinThreshold : nat,
      LiveMajorDue OldLiveNext
        (RearmedWatermark OldLiveAfter MinThreshold)
        MinThreshold ->
      OldLiveNext > MinThreshold.
  Proof.
    intros OldLiveAfter OldLiveNext MinThreshold Hdue.
    unfold LiveMajorDue, RearmedWatermark in Hdue.
    assert (MinThreshold <=
            Nat.max (Nat.max (OldLiveAfter + OldLiveAfter) MinThreshold)
                    MinThreshold) by apply Nat.le_max_r.
    lia.
  Qed.

  Theorem rearmed_watermark_at_least_min_threshold :
    forall OldLiveAfter MinThreshold : nat,
      MinThreshold <= RearmedWatermark OldLiveAfter MinThreshold.
  Proof.
    intros OldLiveAfter MinThreshold.
    unfold RearmedWatermark.
    apply Nat.le_max_r.
  Qed.
End MajorWatermarkRearmModel.

End MeTTaTron_GC_MajorWatermarkRearm.
