(** Rocq model of the R-FL free-list lifecycle in the CESK index arena.

    The persistent free bit is the authority for whether an address is already
    present in the free list. Push/pop/drain preserve the invariant that the bit
    and list membership agree, and that the free list has no duplicates.
*)

From Stdlib Require Import Bool List.
Import ListNotations.

Module MeTTaTron_GC_FreeList.

Section FreeListModel.
  Variable Addr : Type.
  Variable Addr_eq_dec : forall x y : Addr, {x = y} + {x <> y}.

  Record State : Type := {
    freeList : list Addr;
    freeBit : Addr -> bool;
  }.

  Definition Valid (st : State) : Prop :=
    (forall a, freeBit st a = true <-> In a (freeList st)) /\
    NoDup (freeList st).

  Definition set_bit (a : Addr) (v : bool) (bits : Addr -> bool) : Addr -> bool :=
    fun x => if Addr_eq_dec x a then v else bits x.

  Lemma set_bit_eq :
    forall (a : Addr) (v : bool) (bits : Addr -> bool),
      set_bit a v bits a = v.
  Proof.
    intros a v bits.
    unfold set_bit.
    destruct (Addr_eq_dec a a) as [_ | Hneq].
    - reflexivity.
    - contradiction.
  Qed.

  Lemma set_bit_neq :
    forall (a x : Addr) (v : bool) (bits : Addr -> bool),
      x <> a -> set_bit a v bits x = bits x.
  Proof.
    intros a x v bits Hneq.
    unfold set_bit.
    destruct (Addr_eq_dec x a) as [Heq | _].
    - contradiction.
    - reflexivity.
  Qed.

  Definition push (a : Addr) (st : State) : State :=
    if freeBit st a then
      st
    else
      {| freeList := a :: freeList st;
         freeBit := set_bit a true (freeBit st) |}.

  Definition pop (st : State) : State :=
    match freeList st with
    | [] => st
    | a :: rest =>
        {| freeList := rest;
           freeBit := set_bit a false (freeBit st) |}
    end.

  Definition majorDrain (_st : State) : State :=
    {| freeList := [];
       freeBit := fun _ => false |}.

  Definition drainReleasedSegment (released : Addr -> bool) (st : State) : State :=
    {| freeList := filter (fun a => negb (released a)) (freeList st);
       freeBit := fun a => if released a then false else freeBit st a |}.

  Theorem push_preserves_valid :
    forall (a : Addr) (st : State), Valid st -> Valid (push a st).
  Proof.
    intros a st [Hbit Hnodup].
    unfold push.
    destruct (freeBit st a) eqn:Hfree.
    - split; assumption.
    - split.
      + intro x.
        destruct (Addr_eq_dec x a) as [Heq | Hneq].
        * subst x.
          simpl.
          split.
          -- intro Htrue.
             left.
             reflexivity.
          -- intro Hin.
             apply set_bit_eq.
        * split.
          -- intro Hx.
             simpl in Hx.
             rewrite set_bit_neq in Hx by exact Hneq.
             right.
             apply Hbit.
             exact Hx.
          -- intro Hx.
             simpl in Hx.
             destruct Hx as [Heq | Hin].
             ++ exfalso.
                apply Hneq.
                symmetry.
                exact Heq.
             ++ simpl.
                rewrite set_bit_neq by exact Hneq.
                apply Hbit.
                exact Hin.
      + constructor.
        * intro Hin.
          pose proof (proj2 (Hbit a) Hin) as Hfree'.
          rewrite Hfree in Hfree'.
          discriminate.
        * exact Hnodup.
  Qed.

  Theorem pop_preserves_valid :
    forall st : State, Valid st -> Valid (pop st).
  Proof.
    intros [fl bits] [Hbit Hnodup].
    simpl in *.
    destruct fl as [| head rest].
    - split; assumption.
    - inversion Hnodup as [| ? ? Hhead_notin Hrest_nodup]; subst.
      split.
      + intro x.
        destruct (Addr_eq_dec x head) as [Heq | Hneq].
        * subst x.
          simpl.
          split.
          -- intro Hfalse.
             rewrite set_bit_eq in Hfalse.
             discriminate Hfalse.
          -- intro Hin.
             contradiction.
        * split.
          -- intro Hx.
             simpl in Hx.
             rewrite set_bit_neq in Hx by exact Hneq.
             pose proof (proj1 (Hbit x) Hx) as Hin.
             simpl in Hin.
             destruct Hin as [Heq | Hin].
             ++ exfalso.
                apply Hneq.
                symmetry.
                exact Heq.
             ++ exact Hin.
          -- intro Hin.
             simpl.
             rewrite set_bit_neq by exact Hneq.
             apply Hbit.
             simpl.
             right.
             exact Hin.
      + exact Hrest_nodup.
  Qed.

  Theorem majorDrain_valid :
    forall st : State, Valid (majorDrain st).
  Proof.
    intro st.
    unfold Valid, majorDrain.
    simpl.
    split.
    - intro a.
      split.
      + intro Hfalse.
        discriminate Hfalse.
      + intro Hin.
        contradiction.
    - constructor.
  Qed.

  Theorem drainReleasedSegment_preserves_valid :
    forall (released : Addr -> bool) (st : State),
      Valid st -> Valid (drainReleasedSegment released st).
  Proof.
    intros released st [Hbit Hnodup].
    unfold Valid, drainReleasedSegment.
    simpl.
    split.
    - intro a.
      destruct (released a) eqn:Hrel.
      + split.
        * intro Hfalse.
          discriminate Hfalse.
        * intro Hin.
          apply filter_In in Hin as [_ Hkeep].
          rewrite Hrel in Hkeep.
          discriminate Hkeep.
      + split.
        * intro Hb.
          apply filter_In.
          split.
          -- apply Hbit.
             exact Hb.
          -- rewrite Hrel.
             reflexivity.
        * intro Hin.
          apply filter_In in Hin as [Hin _].
          apply Hbit.
          exact Hin.
    - apply NoDup_filter.
      exact Hnodup.
  Qed.
End FreeListModel.

End MeTTaTron_GC_FreeList.
