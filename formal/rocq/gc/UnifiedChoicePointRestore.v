(** E3 unified Selective CESK* backtrack/restore safety.

    This proof composes the production re-enterable continuation families:

      - stored lazy branch coroutines,
      - VM choice-point spine handles,
      - JIT choice-point ABI spine owner slots bridged to ContinuationAddr,
      - trampoline fan-out spine handles.

    Each family has its own implementation proof because the concrete payloads
    and ABI constraints differ. This file proves the aggregate E3 obligation:
    once a live production family is classified as re-enterable, every value a
    future resume/fail/backtrack transition can touch is rooted before sweep,
    the transition restores from the family carrier, and the carrier does not
    leave a stale live node after restore/pop.
*)

Module MeTTaTron_GC_UnifiedChoicePointRestore.

Inductive ReenterableFamily : Type :=
| LazyBranchCoroutine
| VmChoicePoint
| JitChoicePoint
| TrampolineFanout.

Section UnifiedRestoreModel.
  Variable Handle Value : Type.

  Variable Live : ReenterableFamily -> Handle -> Prop.
  Variable FutureTouch : ReenterableFamily -> Handle -> Value -> Prop.
  Variable RootedValue : Value -> Prop.
  Variable FreedValue : Value -> Prop.
  Variable RestoresFromCarrier : ReenterableFamily -> Handle -> Prop.
  Variable StaleAfterRestore : ReenterableFamily -> Handle -> Prop.

  Definition FamilyRootsFutureTouch (family : ReenterableFamily) : Prop :=
    forall h v,
      Live family h ->
      FutureTouch family h v ->
      RootedValue v.

  Definition FamilyRestoreComplete (family : ReenterableFamily) : Prop :=
    forall h,
      Live family h ->
      RestoresFromCarrier family h /\ ~ StaleAfterRestore family h.

  Inductive UnifiedLive (h : Handle) : Prop :=
  | unified_lazy :
      Live LazyBranchCoroutine h -> UnifiedLive h
  | unified_vm :
      Live VmChoicePoint h -> UnifiedLive h
  | unified_jit :
      Live JitChoicePoint h -> UnifiedLive h
  | unified_trampoline :
      Live TrampolineFanout h -> UnifiedLive h.

  Inductive UnifiedFutureTouch (h : Handle) (v : Value) : Prop :=
  | unified_future_lazy :
      Live LazyBranchCoroutine h ->
      FutureTouch LazyBranchCoroutine h v ->
      UnifiedFutureTouch h v
  | unified_future_vm :
      Live VmChoicePoint h ->
      FutureTouch VmChoicePoint h v ->
      UnifiedFutureTouch h v
  | unified_future_jit :
      Live JitChoicePoint h ->
      FutureTouch JitChoicePoint h v ->
      UnifiedFutureTouch h v
  | unified_future_trampoline :
      Live TrampolineFanout h ->
      FutureTouch TrampolineFanout h v ->
      UnifiedFutureTouch h v.

  Definition AllFamiliesRootFutureTouches : Prop :=
    FamilyRootsFutureTouch LazyBranchCoroutine /\
    FamilyRootsFutureTouch VmChoicePoint /\
    FamilyRootsFutureTouch JitChoicePoint /\
    FamilyRootsFutureTouch TrampolineFanout.

  Definition AllFamiliesRestoreComplete : Prop :=
    FamilyRestoreComplete LazyBranchCoroutine /\
    FamilyRestoreComplete VmChoicePoint /\
    FamilyRestoreComplete JitChoicePoint /\
    FamilyRestoreComplete TrampolineFanout.

  Theorem unified_choice_point_roots_future_touch :
    AllFamiliesRootFutureTouches ->
    forall h v,
      UnifiedFutureTouch h v ->
      RootedValue v.
  Proof.
    intros Hroots h v Hfuture.
    destruct Hroots as [Hlazy [Hvm [Hjit Htrampoline]]].
    destruct Hfuture as
      [Hlive Htouch | Hlive Htouch | Hlive Htouch | Hlive Htouch].
    - apply (Hlazy h v Hlive Htouch).
    - apply (Hvm h v Hlive Htouch).
    - apply (Hjit h v Hlive Htouch).
    - apply (Htrampoline h v Hlive Htouch).
  Qed.

  Theorem unified_choice_point_future_touch_not_freed :
    AllFamiliesRootFutureTouches ->
    (forall v, FreedValue v -> ~ RootedValue v) ->
    forall h v,
      UnifiedFutureTouch h v ->
      ~ FreedValue v.
  Proof.
    intros Hroots Hfreed h v Hfuture Hfreed_v.
    apply (Hfreed v Hfreed_v).
    apply (unified_choice_point_roots_future_touch Hroots h v Hfuture).
  Qed.

  Theorem unified_choice_point_restore_complete :
    AllFamiliesRestoreComplete ->
    forall family h,
      Live family h ->
      RestoresFromCarrier family h /\ ~ StaleAfterRestore family h.
  Proof.
    intros Hrestore family h Hlive.
    destruct Hrestore as [Hlazy [Hvm [Hjit Htrampoline]]].
    destruct family.
    - apply Hlazy. exact Hlive.
    - apply Hvm. exact Hlive.
    - apply Hjit. exact Hlive.
    - apply Htrampoline. exact Hlive.
  Qed.

  Theorem unified_choice_point_root_then_restore_safe :
    AllFamiliesRootFutureTouches ->
    AllFamiliesRestoreComplete ->
    (forall v, FreedValue v -> ~ RootedValue v) ->
    forall family h v,
      Live family h ->
      FutureTouch family h v ->
      RootedValue v /\
      RestoresFromCarrier family h /\
      ~ StaleAfterRestore family h /\
      ~ FreedValue v.
  Proof.
    intros Hroots Hrestore Hfreed family h v Hlive Htouch.
    pose proof (unified_choice_point_restore_complete
                  Hrestore family h Hlive) as [Hcarrier Hnostale].
    repeat split.
    - destruct Hroots as [Hlazy [Hvm [Hjit Htrampoline]]].
      destruct family.
      + apply (Hlazy h v Hlive Htouch).
      + apply (Hvm h v Hlive Htouch).
      + apply (Hjit h v Hlive Htouch).
      + apply (Htrampoline h v Hlive Htouch).
    - exact Hcarrier.
    - exact Hnostale.
    - intros Hfreed_v.
      apply (Hfreed v Hfreed_v).
      destruct Hroots as [Hlazy [Hvm [Hjit Htrampoline]]].
      destruct family.
      + apply (Hlazy h v Hlive Htouch).
      + apply (Hvm h v Hlive Htouch).
      + apply (Hjit h v Hlive Htouch).
      + apply (Htrampoline h v Hlive Htouch).
  Qed.

End UnifiedRestoreModel.

End MeTTaTron_GC_UnifiedChoicePointRestore.
