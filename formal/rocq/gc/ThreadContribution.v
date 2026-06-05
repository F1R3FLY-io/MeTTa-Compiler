(** Per-mutator thread-contribution obligations for the CESK rendezvous
    collector.

    The dedicated collector never reads another mutator's thread-local machine
    registers directly. Each mutator publishes a structural contribution into the
    worker-root buffer. This model states the proof obligation for that
    publication: the concrete reader must include every component of the
    mutator's reachable CESK state, and the driver must drain, mark, and sweep
    from that publication.
*)

Module MeTTaTron_GC_ThreadContribution.

Section ThreadContributionModel.
  Variable Addr : Type.

  Definition TrampolineContributionRoot
      (Extra SCK Env0 Global KSpine Deferred : Addr -> Prop)
      (a : Addr) : Prop :=
    Extra a \/ SCK a \/ Env0 a \/ Global a \/ KSpine a \/ Deferred a.

  Definition TierLeafContributionRoot
      (Extra Global KSpine : Addr -> Prop)
      (a : Addr) : Prop :=
    Extra a \/ Global a \/ KSpine a.

  Theorem trampoline_component_in_thread_root :
    forall (Extra SCK Env0 Global KSpine Deferred ThreadRoot : Addr -> Prop),
      (forall a, Extra a -> ThreadRoot a) ->
      (forall a, SCK a -> ThreadRoot a) ->
      (forall a, Env0 a -> ThreadRoot a) ->
      (forall a, Global a -> ThreadRoot a) ->
      (forall a, KSpine a -> ThreadRoot a) ->
      (forall a, Deferred a -> ThreadRoot a) ->
      forall a,
        TrampolineContributionRoot Extra SCK Env0 Global KSpine Deferred a ->
        ThreadRoot a.
  Proof.
    intros Extra SCK Env0 Global KSpine Deferred ThreadRoot
           Hextra Hsck Henv0 Hglobal Hk Hdeferred a Hroot.
    destruct Hroot as
      [Hextra_a |
       [Hsck_a |
        [Henv0_a |
         [Hglobal_a |
          [Hk_a | Hdeferred_a]]]]].
    - apply Hextra; exact Hextra_a.
    - apply Hsck; exact Hsck_a.
    - apply Henv0; exact Henv0_a.
    - apply Hglobal; exact Hglobal_a.
    - apply Hk; exact Hk_a.
    - apply Hdeferred; exact Hdeferred_a.
  Qed.

  Theorem tier_leaf_component_in_thread_root :
    forall (Extra Global KSpine ThreadRoot : Addr -> Prop),
      (forall a, Extra a -> ThreadRoot a) ->
      (forall a, Global a -> ThreadRoot a) ->
      (forall a, KSpine a -> ThreadRoot a) ->
      forall a,
        TierLeafContributionRoot Extra Global KSpine a ->
        ThreadRoot a.
  Proof.
    intros Extra Global KSpine ThreadRoot Hextra Hglobal Hk a Hroot.
    destruct Hroot as [Hextra_a | [Hglobal_a | Hk_a]].
    - apply Hextra; exact Hextra_a.
    - apply Hglobal; exact Hglobal_a.
    - apply Hk; exact Hk_a.
  Qed.

  Theorem published_thread_contribution_survives_collection :
    forall (ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop),
      (forall a, ThreadRoot a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, ThreadRoot a -> ~ Freed a.
  Proof.
    intros ThreadRoot BufferRoot DriverRoot Marked Freed
           Hpublish Hdrain Hmark Hsweep a Hthread Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hdrain.
    apply Hpublish.
    exact Hthread.
  Qed.

  Theorem trampoline_contribution_survives_collection :
    forall (Extra SCK Env0 Global KSpine Deferred
            ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop),
      (forall a, Extra a -> ThreadRoot a) ->
      (forall a, SCK a -> ThreadRoot a) ->
      (forall a, Env0 a -> ThreadRoot a) ->
      (forall a, Global a -> ThreadRoot a) ->
      (forall a, KSpine a -> ThreadRoot a) ->
      (forall a, Deferred a -> ThreadRoot a) ->
      (forall a, ThreadRoot a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        TrampolineContributionRoot Extra SCK Env0 Global KSpine Deferred a ->
        ~ Freed a.
  Proof.
    intros Extra SCK Env0 Global KSpine Deferred
           ThreadRoot BufferRoot DriverRoot Marked Freed
           Hextra Hsck Henv0 Hglobal Hk Hdeferred Hpublish Hdrain Hmark Hsweep
           a Hcomponent.
    apply (published_thread_contribution_survives_collection
             ThreadRoot BufferRoot DriverRoot Marked Freed
             Hpublish Hdrain Hmark Hsweep a).
    apply (trampoline_component_in_thread_root
             Extra SCK Env0 Global KSpine Deferred ThreadRoot
             Hextra Hsck Henv0 Hglobal Hk Hdeferred a).
    exact Hcomponent.
  Qed.

  Theorem tier_leaf_contribution_survives_collection :
    forall (Extra Global KSpine
            ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop),
      (forall a, Extra a -> ThreadRoot a) ->
      (forall a, Global a -> ThreadRoot a) ->
      (forall a, KSpine a -> ThreadRoot a) ->
      (forall a, ThreadRoot a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        TierLeafContributionRoot Extra Global KSpine a ->
        ~ Freed a.
  Proof.
    intros Extra Global KSpine ThreadRoot BufferRoot DriverRoot Marked Freed
           Hextra Hglobal Hk Hpublish Hdrain Hmark Hsweep a Hcomponent.
    apply (published_thread_contribution_survives_collection
             ThreadRoot BufferRoot DriverRoot Marked Freed
             Hpublish Hdrain Hmark Hsweep a).
    apply (tier_leaf_component_in_thread_root
             Extra Global KSpine ThreadRoot
             Hextra Hglobal Hk a).
    exact Hcomponent.
  Qed.
End ThreadContributionModel.

End MeTTaTron_GC_ThreadContribution.
