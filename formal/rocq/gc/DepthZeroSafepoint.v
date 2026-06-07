(** E1 cooperative-safepoint depth-zero obligations.

    A cooperative tier-leaf safepoint may be reached from code that is not
    currently inside an EvalGuard.  Such a caller is not counted in the
    dedicated collector's participant snapshot, so it must not park or drop an
    EvalGuard.  Depth-positive callers may take the normal park path.
*)

Module MeTTaTron_GC_DepthZeroSafepoint.

Section DepthZeroSafepointModel.
  Definition DepthZeroGuard
      (DepthPositive Park DropGuard : Prop) : Prop :=
    ~ DepthPositive -> ~ Park /\ ~ DropGuard.

  Definition DepthPositiveParkPath
      (DepthPositive Park DropGuard : Prop) : Prop :=
    DepthPositive -> Park /\ DropGuard.

  Theorem depth_zero_does_not_park :
    forall DepthPositive Park DropGuard,
      DepthZeroGuard DepthPositive Park DropGuard ->
      ~ DepthPositive ->
      ~ Park.
  Proof.
    intros DepthPositive Park DropGuard Hguard Hzero.
    destruct (Hguard Hzero) as [Hno_park _].
    exact Hno_park.
  Qed.

  Theorem depth_zero_does_not_drop_guard :
    forall DepthPositive Park DropGuard,
      DepthZeroGuard DepthPositive Park DropGuard ->
      ~ DepthPositive ->
      ~ DropGuard.
  Proof.
    intros DepthPositive Park DropGuard Hguard Hzero.
    destruct (Hguard Hzero) as [_ Hno_drop].
    exact Hno_drop.
  Qed.

  Theorem depth_zero_safepoint_has_no_invalid_participation :
    forall DepthPositive Park DropGuard,
      DepthZeroGuard DepthPositive Park DropGuard ->
      ~ DepthPositive ->
      (Park \/ DropGuard) ->
      False.
  Proof.
    intros DepthPositive Park DropGuard Hguard Hzero Hinvalid.
    destruct Hinvalid as [Hpark | Hdrop].
    - apply (depth_zero_does_not_park DepthPositive Park DropGuard Hguard Hzero).
      exact Hpark.
    - apply (depth_zero_does_not_drop_guard DepthPositive Park DropGuard Hguard Hzero).
      exact Hdrop.
  Qed.

  Theorem depth_positive_safepoint_uses_park_path :
    forall DepthPositive Park DropGuard,
      DepthPositiveParkPath DepthPositive Park DropGuard ->
      DepthPositive ->
      Park /\ DropGuard.
  Proof.
    intros DepthPositive Park DropGuard Hpath Hpositive.
    apply Hpath.
    exact Hpositive.
  Qed.
End DepthZeroSafepointModel.

End MeTTaTron_GC_DepthZeroSafepoint.
