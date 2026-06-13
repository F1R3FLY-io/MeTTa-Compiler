(** JIT cache-entry thread-safety obligation.

    The runtime fix removes manual unsafe Send/Sync impls from CacheEntry by
    replacing the raw native-code data pointer field with a typed native
    function pointer. Source coupling pins that concrete field shape and rejects
    reintroducing the manual impls. *)

Module MeTTaTron_GC_JitCacheEntryThreadSafety.

Section FieldAutotraits.
  Definition CacheEntryFieldsThreadSafe
      (NativeCodeFnSafe CodeSizeSafe ProfileArcSafe TierSafe InstantSafe : Prop) : Prop :=
    NativeCodeFnSafe /\ CodeSizeSafe /\ ProfileArcSafe /\ TierSafe /\ InstantSafe.

  Definition ManualUnsafeImplFree (ManualSendImpl ManualSyncImpl : Prop) : Prop :=
    ~ ManualSendImpl /\ ~ ManualSyncImpl.

  Definition CacheEntryThreadSafe
      (NativeCodeFnSafe CodeSizeSafe ProfileArcSafe TierSafe InstantSafe
       ManualSendImpl ManualSyncImpl : Prop) : Prop :=
    CacheEntryFieldsThreadSafe
      NativeCodeFnSafe CodeSizeSafe ProfileArcSafe TierSafe InstantSafe /\
    ManualUnsafeImplFree ManualSendImpl ManualSyncImpl.

  Theorem field_autotraits_make_cache_entry_thread_safe :
    forall (NativeCodeFnSafe CodeSizeSafe ProfileArcSafe TierSafe InstantSafe
            ManualSendImpl ManualSyncImpl : Prop),
      NativeCodeFnSafe ->
      CodeSizeSafe ->
      ProfileArcSafe ->
      TierSafe ->
      InstantSafe ->
      ~ ManualSendImpl ->
      ~ ManualSyncImpl ->
      CacheEntryThreadSafe
        NativeCodeFnSafe CodeSizeSafe ProfileArcSafe TierSafe InstantSafe
        ManualSendImpl ManualSyncImpl.
  Proof.
    intros NativeCodeFnSafe CodeSizeSafe ProfileArcSafe TierSafe InstantSafe
           ManualSendImpl ManualSyncImpl
           Hnative Hsize Hprofile Htier Hinstant Hno_send Hno_sync.
    split.
    - repeat split; assumption.
    - split; assumption.
  Qed.

  Theorem manual_impl_reintroduction_breaks_gate :
    forall (ManualSendImpl ManualSyncImpl : Prop),
      ManualUnsafeImplFree ManualSendImpl ManualSyncImpl ->
      ManualSendImpl \/ ManualSyncImpl ->
      False.
  Proof.
    intros ManualSendImpl ManualSyncImpl Hfree Hmanual.
    destruct Hfree as [Hno_send Hno_sync].
    destruct Hmanual as [Hsend | Hsync].
    - apply Hno_send. exact Hsend.
    - apply Hno_sync. exact Hsync.
  Qed.
End FieldAutotraits.

End MeTTaTron_GC_JitCacheEntryThreadSafety.
