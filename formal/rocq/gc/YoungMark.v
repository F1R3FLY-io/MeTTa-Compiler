(** Rocq model of the young-generation minor-mark soundness obligation. *)

From Stdlib Require Import Arith.

Module MeTTaTron_GC_YoungMark.

Section YoungMarkModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Theorem bump_order_no_old_to_young :
    forall (seg : Addr -> nat) (floor : nat) (Edge : Addr -> Addr -> Prop),
      (forall parent child, Edge parent child -> seg child <= seg parent) ->
      forall parent child,
        Edge parent child -> floor <= seg child -> floor <= seg parent.
  Proof.
    intros seg floor Edge Horder parent child Hedge Hyoung.
    eapply Nat.le_trans.
    - exact Hyoung.
    - apply Horder.
      exact Hedge.
  Qed.

  Theorem young_reachable_marked :
    forall (Root Young Marked : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Young a -> Marked a) ->
      (forall parent child, Edge parent child -> Young child -> Young parent) ->
      (forall parent child,
          Marked parent -> Young parent -> Edge parent child -> Young child -> Marked child) ->
      forall a, Reach Root Edge a -> Young a -> Marked a.
  Proof.
    intros Root Young Marked Edge Hroot Hno_old_to_young Hclosed a Hreach.
    induction Hreach as [a Hroot_a | parent child Hparent IH Hedge].
    - intro Hyoung.
      apply Hroot; assumption.
    - intro Hchild_young.
      pose proof (Hno_old_to_young parent child Hedge Hchild_young) as Hparent_young.
      apply Hclosed with (parent := parent); auto.
  Qed.

  Definition MinorRetains (Young Marked : Addr -> Prop) (a : Addr) : Prop :=
    Young a -> Marked a.

  Theorem minor_retains_reachable :
    forall (Root Young Marked : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Young a -> Marked a) ->
      (forall parent child, Edge parent child -> Young child -> Young parent) ->
      (forall parent child,
          Marked parent -> Young parent -> Edge parent child -> Young child -> Marked child) ->
      forall a, Reach Root Edge a -> MinorRetains Young Marked a.
  Proof.
    intros Root Young Marked Edge Hroot Hno_old_to_young Hclosed a Hreach Hyoung.
    eapply young_reachable_marked; eauto.
  Qed.
End YoungMarkModel.

End MeTTaTron_GC_YoungMark.
