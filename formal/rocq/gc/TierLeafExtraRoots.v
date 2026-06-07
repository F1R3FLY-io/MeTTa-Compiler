(** Tier-leaf VM/JIT register roots for the CESK rendezvous collector.

    VM and JIT code can reach the collector while live values are held in
    tier-local register files rather than in the trampoline S/C/K vectors. The
    mutator must publish those values as the [extra] part of its tier-leaf
    contribution before it parks. This module proves the non-TLC safety shape
    for that publication.
*)

Module MeTTaTron_GC_TierLeafExtraRoots.

Section TierLeafExtraRootsModel.
  Variable Addr : Type.

  Inductive VmRegisterRoot
      (ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot BindingStackRoot
       CallFrameRoot ChoicePointRoot CollapseFrameRoot CollapseBindFrameRoot
       PerResultBindingRoot DispatchMemoRoot TrailRoot ExpectedTypeRoot :
          Addr -> Prop) : Addr -> Prop :=
  | vm_chunk :
      forall a, ChunkRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_value_stack :
      forall a, ValueStackRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_local :
      forall a, LocalRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_current_binding :
      forall a, CurrentBindingRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_binding_stack :
      forall a, BindingStackRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_call_frame :
      forall a, CallFrameRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_choice_point :
      forall a, ChoicePointRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_collapse_frame :
      forall a, CollapseFrameRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_collapse_bind_frame :
      forall a, CollapseBindFrameRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_per_result_binding :
      forall a, PerResultBindingRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_dispatch_memo :
      forall a, DispatchMemoRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_trail :
      forall a, TrailRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a
  | vm_expected_type :
      forall a, ExpectedTypeRoot a ->
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a.

  Inductive JitRegisterRoot
      (HotRoot LiteralPoolRoot ArenaLiteralPoolRoot CurrentChunkRoot
       ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot BindingFrameRoot
       TemplateResultRoot StateCacheRoot : Addr -> Prop) : Addr -> Prop :=
  | jit_hot :
      forall a, HotRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_literal_pool :
      forall a, LiteralPoolRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_arena_literal_pool :
      forall a, ArenaLiteralPoolRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_current_chunk :
      forall a, CurrentChunkRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_value_stack :
      forall a, ValueStackRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_result :
      forall a, ResultRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_saved_stack :
      forall a, SavedStackRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_choice_point :
      forall a, ChoicePointRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_binding_frame :
      forall a, BindingFrameRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_template_result :
      forall a, TemplateResultRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | jit_state_cache :
      forall a, StateCacheRoot a ->
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
          BindingFrameRoot TemplateResultRoot StateCacheRoot a.

  Theorem vm_register_root_in_extra :
    forall (ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot BindingStackRoot
            CallFrameRoot ChoicePointRoot CollapseFrameRoot CollapseBindFrameRoot
            PerResultBindingRoot DispatchMemoRoot TrailRoot ExpectedTypeRoot
            Extra : Addr -> Prop),
      (forall a, ChunkRoot a -> Extra a) ->
      (forall a, ValueStackRoot a -> Extra a) ->
      (forall a, LocalRoot a -> Extra a) ->
      (forall a, CurrentBindingRoot a -> Extra a) ->
      (forall a, BindingStackRoot a -> Extra a) ->
      (forall a, CallFrameRoot a -> Extra a) ->
      (forall a, ChoicePointRoot a -> Extra a) ->
      (forall a, CollapseFrameRoot a -> Extra a) ->
      (forall a, CollapseBindFrameRoot a -> Extra a) ->
      (forall a, PerResultBindingRoot a -> Extra a) ->
      (forall a, DispatchMemoRoot a -> Extra a) ->
      (forall a, TrailRoot a -> Extra a) ->
      (forall a, ExpectedTypeRoot a -> Extra a) ->
      forall a,
        VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
          BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
          CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
          ExpectedTypeRoot a ->
        Extra a.
  Proof.
    intros ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot BindingStackRoot
           CallFrameRoot ChoicePointRoot CollapseFrameRoot CollapseBindFrameRoot
           PerResultBindingRoot DispatchMemoRoot TrailRoot ExpectedTypeRoot Extra
           Hchunk Hstack Hlocal Hcurrent Hbindings Hcall Hchoice Hcollapse
           Hcollapse_bind Hper_result Hmemo Htrail Hexpected a Hroot.
    destruct Hroot.
    - apply Hchunk. exact H.
    - apply Hstack. exact H.
    - apply Hlocal. exact H.
    - apply Hcurrent. exact H.
    - apply Hbindings. exact H.
    - apply Hcall. exact H.
    - apply Hchoice. exact H.
    - apply Hcollapse. exact H.
    - apply Hcollapse_bind. exact H.
    - apply Hper_result. exact H.
    - apply Hmemo. exact H.
    - apply Htrail. exact H.
    - apply Hexpected. exact H.
  Qed.

  Theorem jit_register_root_in_extra :
    forall (HotRoot LiteralPoolRoot ArenaLiteralPoolRoot CurrentChunkRoot
            ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
            BindingFrameRoot TemplateResultRoot StateCacheRoot Extra :
              Addr -> Prop),
      (forall a, HotRoot a -> Extra a) ->
      (forall a, LiteralPoolRoot a -> Extra a) ->
      (forall a, ArenaLiteralPoolRoot a -> Extra a) ->
      (forall a, CurrentChunkRoot a -> Extra a) ->
      (forall a, ValueStackRoot a -> Extra a) ->
      (forall a, ResultRoot a -> Extra a) ->
      (forall a, SavedStackRoot a -> Extra a) ->
      (forall a, ChoicePointRoot a -> Extra a) ->
      (forall a, BindingFrameRoot a -> Extra a) ->
      (forall a, TemplateResultRoot a -> Extra a) ->
      (forall a, StateCacheRoot a -> Extra a) ->
      forall a,
        JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
          CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot
          ChoicePointRoot BindingFrameRoot TemplateResultRoot StateCacheRoot a ->
        Extra a.
  Proof.
    intros HotRoot LiteralPoolRoot ArenaLiteralPoolRoot CurrentChunkRoot
           ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
           BindingFrameRoot TemplateResultRoot StateCacheRoot Extra
           Hhot Hliteral Harena Hchunk Hstack Hresult Hsaved Hchoice Hbinding
           Htemplate Hstate a Hroot.
    destruct Hroot.
    - apply Hhot. exact H.
    - apply Hliteral. exact H.
    - apply Harena. exact H.
    - apply Hchunk. exact H.
    - apply Hstack. exact H.
    - apply Hresult. exact H.
    - apply Hsaved. exact H.
    - apply Hchoice. exact H.
    - apply Hbinding. exact H.
    - apply Htemplate. exact H.
    - apply Hstate. exact H.
  Qed.

  Theorem extra_root_survives_collection :
    forall (Extra ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop),
      (forall a, Extra a -> ThreadRoot a) ->
      (forall a, ThreadRoot a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, Extra a -> ~ Freed a.
  Proof.
    intros Extra ThreadRoot BufferRoot DriverRoot Marked Freed
           Hextra Hpublish Hdrain Hmark Hsweep a Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hdrain.
    apply Hpublish.
    apply Hextra.
    exact Hroot.
  Qed.
End TierLeafExtraRootsModel.

End MeTTaTron_GC_TierLeafExtraRoots.
