(** Rocq model of the F1 SATB-young-lever stale-old-mark obligation.

    Source coupling:
      - [IndexArena::mark_from_roots_with] (index_arena.rs) — the PRUNING full
        mark: descent into a node's children happens only when [mark] returns
        "newly marked" (the mark bitmap doubles as the traversal dedup set).
      - [IndexArena::clear_old_marks] (index_arena.rs) + the rendezvous-phase
        minor arm of [mark_sweep_if_over_watermark] (index_heap.rs) — the
        ClearOldMarks=TRUE wiring of tla/SATBYoungSweepStaleOldMark.tla.

    Obligations:
      1. [stale_mark_can_hide_live_child] — the DISCRIMINATOR (the model's
         expected-fail young_only cfg, in source terms): with a pre-set
         ("stale") mark surviving into a cycle, a pruning mark can satisfy its
         closure spec yet leave a reachable node unmarked (the under-mark that
         becomes a use-after-free at the sweep).
      2. [clear_marks_makes_pruning_mark_complete] — the FIX: starting from an
         all-clear mark state (what [clear_old_marks] re-establishes for the
         old generation, and what every full sweep ends with), ANY mark set
         satisfying the pruning-mark closure spec covers the whole reachable
         set.
      3. [final_sweep_clear_old_no_stale] — the direct mirror of the TLA
         model's [FinalSweep] with ClearOldMarks=TRUE preserving
         [NoStaleOldMark] at promotion.

    No assumptions, no axioms, no admits: obligation 1 is a closed constructive
    counterexample; 2 and 3 are closed inductions/case analyses. *)

From Stdlib Require Import Bool.

Module MeTTaTron_GC_StaleOldMarkClear.

Section PruningMark.
  Variable Addr : Type.

  (** Reachability through the collector's edge relation (same shape as
      [MeTTaTron_GC_YoungMark.Reach]). *)
  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  (** The pruning mark's closure spec. [M0] is the mark state at cycle start
      (stale marks included); [M] is the mark state after the mark phase.

      - [pm_root]: every root ends up marked.
      - [pm_keep]: marking only adds bits ([M0 ⊆ M]).
      - [pm_fresh_descend]: the children of a node the marker NEWLY marked
        (marked now, NOT marked at cycle start) are marked — this is the ONLY
        descent guarantee [mark_from_roots_with] gives: `if self.mark(k)`
        pushes [k] to the worklist exactly when the bit was previously unset.

      A node already marked at cycle start gives the marker NO obligation to
      visit its children — that is precisely the prune. *)
  Record PruningMark
    (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop)
    (M0 M : Addr -> Prop) : Prop := {
    pm_root : forall a, Root a -> M a;
    pm_keep : forall a, M0 a -> M a;
    pm_fresh_descend :
      forall p c, M p -> ~ M0 p -> Edge p c -> M c
  }.

  (** Obligation 2 (the FIX): from an all-clear start state, any pruning mark
      is COMPLETE — every reachable node is marked. This is why
      [clear_old_marks] (re-establishing all-clear for the old generation
      after a rendezvous minor) makes the NEXT full mark sound, and why every
      full sweep (which clears every swept segment's marks) does the same. *)
  Theorem clear_marks_makes_pruning_mark_complete :
    forall (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) (M : Addr -> Prop),
      PruningMark Root Edge (fun _ => False) M ->
      forall a, Reach Root Edge a -> M a.
  Proof.
    intros Root Edge M [Hroot _ Hfresh] a Hreach.
    induction Hreach as [a Hroot_a | p c Hp IH Hedge].
    - apply Hroot; assumption.
    - apply (Hfresh p c).
      + exact IH.
      + intro Hfalse. exact Hfalse.
      + exact Hedge.
  Qed.

End PruningMark.

(** Obligation 1 (the DISCRIMINATOR): a concrete two-node heap
    [parent --Edge--> child], [Root = {parent}], where the stale start state
    [M0 = {parent}] admits a pruning mark [M = {parent}] that satisfies the
    full closure spec yet leaves the reachable [child] unmarked. With
    [M0 = ∅] this [M] would violate [pm_fresh_descend]; the stale bit is
    exactly what licenses the under-mark. *)
Section Discriminator.

  (* Addr := bool; parent := true; child := false. *)
  Definition DRoot (a : bool) : Prop := a = true.
  Definition DEdge (p c : bool) : Prop := p = true /\ c = false.
  Definition DM0 (a : bool) : Prop := a = true.   (* the stale mark *)
  Definition DM (a : bool) : Prop := a = true.    (* mark phase adds nothing *)

  Theorem stale_mark_can_hide_live_child :
    PruningMark bool DRoot DEdge DM0 DM
    /\ Reach bool DRoot DEdge false
    /\ ~ DM false.
  Proof.
    split; [| split].
    - constructor.
      + intros a H. exact H.
      + intros a H. exact H.
      + (* No node is freshly marked (M ⊆ M0), so the descent premise
           [M p /\ ~ M0 p] is contradictory — the prune in pure form. *)
        intros p c HM HnotM0 _.
        exfalso. apply HnotM0. exact HM.
    - apply (reach_step bool DRoot DEdge true false).
      + apply reach_root. reflexivity.
      + split; reflexivity.
    - unfold DM. discriminate.
  Qed.

End Discriminator.

(** Obligation 3: the direct mirror of tla/SATBYoungSweepStaleOldMark.tla.
    Phases: satb_marked → swept → promoted. [FinalSweep] clears young marks
    and clears old marks iff [clear_old]; [Promote] changes no marks. With
    [clear_old = true], [NoStaleOldMark] (promoted ⇒ ¬oldMarked) holds for
    every run from every initial mark state — the implemented rendezvous-minor
    arm ([sweep_young] + [clear_old_marks]) is the [clear_old = true]
    instance. *)
Section TlaMirror.

  Inductive Phase := SatbMarked | Swept | Promoted.

  Record St := mkSt { phase : Phase; youngMarked : bool; oldMarked : bool }.

  Inductive Step (clear_old : bool) : St -> St -> Prop :=
  | step_final_sweep : forall y o,
      Step clear_old
        (mkSt SatbMarked y o)
        (mkSt Swept false (if clear_old then false else o))
  | step_promote : forall y o,
      Step clear_old (mkSt Swept y o) (mkSt Promoted y o).

  Inductive Run (clear_old : bool) (s0 : St) : St -> Prop :=
  | run_refl : Run clear_old s0 s0
  | run_step : forall s s', Run clear_old s0 s -> Step clear_old s s' -> Run clear_old s0 s'.

  Definition NoStaleOldMark (s : St) : Prop :=
    phase s = Promoted -> oldMarked s = false.

  Theorem final_sweep_clear_old_no_stale :
    forall y0 o0 s,
      Run true (mkSt SatbMarked y0 o0) s ->
      NoStaleOldMark s.
  Proof.
    intros y0 o0 s Hrun.
    (* Invariant: any state reachable from satb_marked under clear_old=true is
       either still satb_marked, or has oldMarked = false. *)
    assert (Hinv : phase s = SatbMarked \/ oldMarked s = false).
    {
      induction Hrun as [| s s' Hrun IH Hstep].
      - left. reflexivity.
      - destruct Hstep as [y o | y o].
        + right. reflexivity.
        + (* Promote preserves marks; the pre-state [Swept] is not SatbMarked,
             so IH gives oldMarked = false. *)
          destruct IH as [Hph | Hold].
          * discriminate Hph.
          * right. exact Hold.
    }
    intro Hpromoted.
    destruct Hinv as [Hph | Hold].
    - rewrite Hpromoted in Hph. discriminate Hph.
    - exact Hold.
  Qed.

End TlaMirror.

End MeTTaTron_GC_StaleOldMarkClear.
