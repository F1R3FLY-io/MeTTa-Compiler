(** E1 dedicated-GC-thread handoff obligations.

    In the quiescence handoff path, `try_drive_blocking` either fails before
    consuming the root vector and returns it for inline collection, or sends the
    vector to the GC thread.  After a successful send, a response-channel failure
    must be treated as "cycle skipped" rather than as an inline fallback: the
    mutator no longer owns the roots.
*)

Module MeTTaTron_GC_DedicatedHandoff.

Section DedicatedHandoffModel.
  Definition InlineFallbackSafe (InlineFallback RootsAvailable : Prop) : Prop :=
    InlineFallback -> RootsAvailable.

  Theorem consumed_roots_forbid_inline_fallback :
    forall Sent RootsAvailable InlineFallback,
      (Sent -> ~ RootsAvailable) ->
      InlineFallbackSafe InlineFallback RootsAvailable ->
      Sent ->
      ~ InlineFallback.
  Proof.
    intros Sent RootsAvailable InlineFallback Hsent_consumes Hinline_safe Hsent Hinline.
    apply (Hsent_consumes Hsent).
    apply Hinline_safe.
    exact Hinline.
  Qed.

  Theorem response_failure_after_send_is_skip_only :
    forall Sent ResponseFailed ReturnedErr ReturnedOkFalse InlineFallback RootsAvailable,
      (Sent -> ~ RootsAvailable) ->
      (ResponseFailed -> ReturnedOkFalse) ->
      (ReturnedOkFalse -> ~ ReturnedErr) ->
      (InlineFallback -> ReturnedErr) ->
      Sent ->
      ResponseFailed ->
      ~ InlineFallback.
  Proof.
    intros Sent ResponseFailed ReturnedErr ReturnedOkFalse InlineFallback RootsAvailable
           _ Hfailed_ok Hok_not_err Hinline_err _ Hfailed Hinline.
    apply Hok_not_err.
    - apply Hfailed_ok.
      exact Hfailed.
    - apply Hinline_err.
      exact Hinline.
  Qed.

  Theorem failed_send_inline_fallback_has_roots :
    forall SendFailed ReturnedErr InlineFallback RootsAvailable,
      (SendFailed -> RootsAvailable) ->
      (SendFailed -> ReturnedErr) ->
      (ReturnedErr -> InlineFallback) ->
      SendFailed ->
      InlineFallbackSafe InlineFallback RootsAvailable ->
      RootsAvailable.
  Proof.
    intros SendFailed ReturnedErr InlineFallback RootsAvailable
           Hfailed_roots _ _ Hfailed _.
    apply Hfailed_roots.
    exact Hfailed.
  Qed.
End DedicatedHandoffModel.

End MeTTaTron_GC_DedicatedHandoff.
